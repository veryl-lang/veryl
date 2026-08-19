//! Which testbench statements can invalidate the design's combinational state.
//!
//! An `initial` block is interleaved with the design's continuous assignments,
//! so the comb must be settled before a statement reads a design signal.
//! Dirtying the comb after every testbench statement is always correct, but it
//! makes a testbench that samples design state into its own variables pay a
//! full settle per sampling statement, though such a write is invisible to the
//! design.
//!
//! A statement only matters to the comb if it writes something the comb can
//! read. `Ir::comb_touched_offsets` is a superset of what the comb touches;
//! anything a testbench writes outside it (its own scratch variables — sampled
//! copies, counters, accumulators) cannot change a comb input, so the settle
//! can be skipped.
//!
//! The filter is sound by construction: a statement is "clean" only when every
//! one of its writes is provably outside that set. Any destination that cannot
//! be resolved to a known variable, and every statement kind whose writes are
//! not enumerable here (compiled chunks, system calls, `$tb` methods), stays
//! dirty.

use crate::HashSet;
use crate::ir::{Ir, Statement, VarOffset};
use crate::testbench::TestbenchStatement;

/// Half-open byte range `[start, end)` in one value buffer, plus the comb
/// verdict shared by every variable in the range.
#[derive(Clone, Copy)]
struct Span {
    start: usize,
    end: usize,
    touched: bool,
}

/// `VERYL_TB_SETTLE_FILTER=0` opts out: every testbench statement settles.
/// A missed settle surfaces far from its cause, so keep the A/B available.
fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("VERYL_TB_SETTLE_FILTER").as_deref() != Ok("0"))
}

/// Statements proven not to invalidate the comb, keyed by the address of the
/// `Statement` inside the slice `build` was given.
///
/// The keys are only meaningful while that exact slice is alive and unmoved;
/// `build` and the run share one local `Vec` in `run_native_testbench_capped`.
/// Absent = dirty, so a statement the builder never visited, or could not
/// prove clean, settles.
#[derive(Default)]
pub(crate) struct TbDirtyFilter {
    clean: HashSet<*const Statement>,
}

impl TbDirtyFilter {
    #[inline]
    pub(crate) fn is_clean(&self, stmt: &Statement) -> bool {
        !self.clean.is_empty() && self.clean.contains(&(stmt as *const Statement))
    }

    /// Classify `stmts` against `ir`'s comb reach.  `stmts` must be the exact
    /// slice later executed; the filter keys on statement addresses.
    pub(crate) fn build(ir: &Ir, stmts: &[TestbenchStatement]) -> Self {
        let mut filter = TbDirtyFilter::default();
        if !enabled() {
            return filter;
        }
        let spans = SpanTable::build(ir);
        let mut clean = HashSet::default();
        collect_clean(stmts, &spans, &mut clean);
        filter.clean = clean;
        filter
    }

    /// Number of statements proved clean, for the tests that pin the filter's
    /// reach.
    #[cfg(test)]
    pub(crate) fn clean_count(&self) -> usize {
        self.clean.len()
    }

    /// `(ff, comb)` span counts, for the test that pins the array-run
    /// coalescing in `SpanTable::build`: one span per array element is correct
    /// but scales with total memory depth.
    #[cfg(test)]
    pub(crate) fn span_counts(ir: &Ir) -> (usize, usize) {
        let spans = SpanTable::build(ir);
        (spans.ff.len(), spans.comb.len())
    }
}

/// Variable spans over both value buffers, sorted so `partition_point` can
/// find the first span overlapping a write.
///
/// The buffer bases are captured by address: both are `Box<[u8]>` owned by the
/// `Ir`, so they stay put for its lifetime.
struct SpanTable {
    ff: Vec<Span>,
    comb: Vec<Span>,
    ff_base: usize,
    ff_len: usize,
    comb_base: usize,
    comb_len: usize,
}

impl SpanTable {
    fn build(ir: &Ir) -> Self {
        let ff_base = ir.ff_values.as_ptr() as usize;
        let ff_end = ff_base + ir.ff_values.len();
        let comb_base = ir.comb_values.as_ptr() as usize;
        let comb_end = comb_base + ir.comb_values.len();
        // A 4-state variable stores its mask bytes directly after the payload,
        // so its span is twice `native_bytes`.
        let stride = |native_bytes: usize| {
            if ir.use_4state {
                native_bytes * 2
            } else {
                native_bytes
            }
        };

        // The touched set is sized by what the design's comb reads, the element
        // count by how deep its memories are, so the per-variable verdict comes
        // from a range query over the sorted set rather than a probe per
        // element.
        let (mut touched_ff, mut touched_comb) = (Vec::new(), Vec::new());
        for o in ir.comb_touched_offsets.iter() {
            match o {
                VarOffset::Ff(x) if *x >= 0 => touched_ff.push(*x as usize),
                VarOffset::Comb(x) if *x >= 0 => touched_comb.push(*x as usize),
                _ => {}
            }
        }
        touched_ff.sort_unstable();
        touched_comb.sort_unstable();
        let any_in = |set: &[usize], lo: usize, hi: usize| -> bool {
            lo < hi && {
                let i = set.partition_point(|&x| x < lo);
                set.get(i).is_some_and(|&x| x < hi)
            }
        };

        let mut ff: Vec<Span> = Vec::new();
        let mut comb: Vec<Span> = Vec::new();
        let mut stack = vec![&ir.module_variables];
        while let Some(vars) = stack.pop() {
            for var in vars.variables.values() {
                // An unpacked array lays its elements out consecutively and
                // shares one `touched` verdict, so the elements are appended
                // as one run rather than one span each.  Any break in the run
                // (a gap, or a jump back) just starts a new one, which the
                // sort below puts back in order.  Runs stop at the variable
                // boundary; `disjoint_cover` rejoins them.
                let len = stride(var.native_bytes);
                let (ff_from, comb_from) = (ff.len(), comb.len());
                // The variable's extent per buffer, for the range query.
                let (mut ff_lo, mut ff_hi) = (usize::MAX, 0usize);
                let (mut comb_lo, mut comb_hi) = (usize::MAX, 0usize);
                for &ptr in var.current_values.iter().chain(var.next_values.iter()) {
                    let p = ptr as usize;
                    let (spans, from, start, lo, hi) = if (ff_base..ff_end).contains(&p) {
                        (&mut ff, ff_from, p - ff_base, &mut ff_lo, &mut ff_hi)
                    } else if (comb_base..comb_end).contains(&p) {
                        (
                            &mut comb,
                            comb_from,
                            p - comb_base,
                            &mut comb_lo,
                            &mut comb_hi,
                        )
                    } else {
                        continue;
                    };
                    *lo = (*lo).min(start);
                    *hi = (*hi).max(start + len);
                    match spans[from..].last_mut() {
                        Some(run) if run.end == start => run.end = start + len,
                        _ => spans.push(Span {
                            start,
                            end: start + len,
                            touched: false,
                        }),
                    }
                }
                // Variable granularity: one element being read is taken to
                // mean the whole variable is, which keeps a base+last array
                // dependency (how a dynamic access is recorded) from leaving
                // its middle elements looking untouched.  Reading the extent
                // rather than the elements can only over-report (a foreign
                // touched offset inside the extent), which costs reach, never
                // soundness.
                let touched =
                    any_in(&touched_ff, ff_lo, ff_hi) || any_in(&touched_comb, comb_lo, comb_hi);
                if touched {
                    for s in ff[ff_from..].iter_mut().chain(comb[comb_from..].iter_mut()) {
                        s.touched = true;
                    }
                }
            }
            for child in &vars.children {
                stack.push(child);
            }
        }
        SpanTable {
            ff: disjoint_cover(ff),
            comb: disjoint_cover(comb),
            ff_base,
            ff_len: ir.ff_values.len(),
            comb_base,
            comb_len: ir.comb_values.len(),
        }
    }

    /// `true` when a write of `len` bytes at `ptr` may reach a comb read.
    /// Unresolvable destinations answer `true` — the caller must stay dirty.
    ///
    /// Relies on `disjoint_cover`: the search below binary-searches `end`,
    /// which only partitions the vector while the spans are disjoint.
    fn write_may_reach_comb(&self, ptr: *mut u8, len: usize) -> bool {
        let p = ptr as usize;
        let (spans, off) = if (self.ff_base..self.ff_base + self.ff_len).contains(&p) {
            (&self.ff, p - self.ff_base)
        } else if (self.comb_base..self.comb_base + self.comb_len).contains(&p) {
            (&self.comb, p - self.comb_base)
        } else {
            return true;
        };
        let write_end = off.saturating_add(len.max(1));
        let mut idx = spans.partition_point(|s| s.end <= off);
        let mut covered = off;
        while idx < spans.len() && spans[idx].start < write_end {
            let s = spans[idx];
            if s.start > covered {
                return true; // a gap: bytes belonging to no known variable
            }
            if s.touched {
                return true;
            }
            covered = covered.max(s.end);
            idx += 1;
        }
        covered < write_end // uncovered bytes past the last known span
    }
}

/// Sort into a disjoint cover, `touched` winning wherever spans overlap.
///
/// Two variables can name the same bytes — an output port and the net it is
/// wired to resolve to one offset — and the run-coalescing above can wrap such
/// a pair in an enclosing run.  `end` is only non-decreasing once the spans are
/// disjoint, which is what the search's binary step needs.  Touching spans are
/// left alone: only a real overlap has to surrender the untouched verdict.
fn disjoint_cover(mut spans: Vec<Span>) -> Vec<Span> {
    spans.sort_by_key(|s| (s.start, s.end));
    let mut out: Vec<Span> = Vec::with_capacity(spans.len());
    for s in spans {
        match out.last_mut() {
            Some(prev) if s.start < prev.end => {
                prev.end = prev.end.max(s.end);
                prev.touched |= s.touched;
            }
            // Runs are cut at variable boundaries while building; rejoin the
            // ones that abut and agree, so the table stays proportional to the
            // storage layout rather than to the variable count.
            Some(prev) if s.start == prev.end && prev.touched == s.touched => {
                prev.end = s.end;
            }
            _ => out.push(s),
        }
    }
    debug_assert!(
        out.windows(2).all(|w| w[0].end <= w[1].start),
        "span cover is not disjoint"
    );
    out
}

/// Record every `Stmt` proved clean.  Container statements are recursed into
/// but never recorded — `exec_one` decides at the leaf.
fn collect_clean(
    stmts: &[TestbenchStatement],
    spans: &SpanTable,
    clean: &mut HashSet<*const Statement>,
) {
    for tb in stmts {
        match tb {
            TestbenchStatement::Stmt(s) => {
                if is_clean_stmt(s, spans) {
                    clean.insert(s as *const Statement);
                }
            }
            TestbenchStatement::If {
                then_block,
                else_block,
                ..
            } => {
                collect_clean(then_block, spans, clean);
                collect_clean(else_block, spans, clean);
            }
            TestbenchStatement::For { body, .. } => collect_clean(body, spans, clean),
            // Clock/reset drive design nets; the rest either write through
            // paths this filter does not model or advance time.  All keep the
            // unconditional dirty mark at their own call sites.
            _ => {}
        }
    }
}

/// A statement is clean when every write it can perform lands outside the
/// comb's reach.  Unknown statement kinds answer `false`.
fn is_clean_stmt(stmt: &Statement, spans: &SpanTable) -> bool {
    match stmt {
        Statement::Assign(a) => !spans.write_may_reach_comb(a.dst, a.dst_native_bytes),
        Statement::AssignDynamic(a) => {
            // The index is dynamic, so every element is a possible write.
            let len =
                (a.dst_stride.unsigned_abs()) * a.dst_num_elements.max(1) + a.dst_native_bytes;
            !spans.write_may_reach_comb(a.dst_base_ptr, len)
        }
        Statement::If(x) => {
            x.true_side.iter().all(|s| is_clean_stmt(s, spans))
                && x.false_side.iter().all(|s| is_clean_stmt(s, spans))
        }
        Statement::Case(x) => {
            x.arms
                .iter()
                .all(|arm| arm.body.iter().all(|s| is_clean_stmt(s, spans)))
                && x.default.iter().all(|s| is_clean_stmt(s, spans))
        }
        Statement::For(x) => {
            !spans.write_may_reach_comb(x.var_ptr, x.var_native_bytes)
                && x.body.iter().all(|s| is_clean_stmt(s, spans))
        }
        Statement::SequentialBlock(body) => body.iter().all(|s| is_clean_stmt(s, spans)),
        Statement::Break => true,
        // A compiled testbench-body chunk carries its write set; every write
        // stays inside one variable, so probing each destination's containing
        // span (len 1) applies the same per-variable verdict as `Assign`.
        Statement::Compiled(c) => match &c.outputs {
            Some(outs) => outs.iter().all(|o| {
                let raw = o.raw();
                if raw < 0 {
                    return false;
                }
                let base = if o.is_ff() {
                    spans.ff_base
                } else {
                    spans.comb_base
                };
                !spans.write_may_reach_comb((base + raw as usize) as *mut u8, 1)
            }),
            None => false,
        },
        // System calls ($display can be clean but $readmemh writes memory)
        // and $tb method calls are not modelled here.
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A nested span must not be able to hide an enclosing touched one.  The
    /// search binary-searches `end`, so an enclosing span sorted before a
    /// narrower one leaves the predicate unpartitioned and the touched span
    /// unvisited; `disjoint_cover` is what rules that shape out.
    #[test]
    fn overlapping_spans_keep_the_touched_verdict() {
        let cover = disjoint_cover(vec![
            Span {
                start: 0,
                end: 16,
                touched: true,
            },
            Span {
                start: 0,
                end: 4,
                touched: true,
            },
            Span {
                start: 4,
                end: 8,
                touched: false,
            },
        ]);
        assert_eq!(cover.len(), 1);
        assert!(cover[0].touched);
        assert_eq!((cover[0].start, cover[0].end), (0, 16));
    }

    /// Adjacent spans describe different variables, so they keep their own
    /// verdicts — only a real overlap merges.
    #[test]
    fn adjacent_spans_that_agree_are_rejoined() {
        // Abutting pieces of one storage region arrive separately and must
        // come back out as one span.
        let span = |start, end, touched| Span {
            start,
            end,
            touched,
        };
        let cover = disjoint_cover(vec![span(8, 16, false), span(0, 8, false)]);
        assert_eq!(cover.len(), 1);
        assert_eq!((cover[0].start, cover[0].end), (0, 16));
        // A gap still separates them.
        let cover = disjoint_cover(vec![span(0, 8, true), span(16, 24, true)]);
        assert_eq!(cover.len(), 2);
    }

    #[test]
    fn adjacent_spans_keep_their_own_verdicts() {
        let cover = disjoint_cover(vec![
            Span {
                start: 0,
                end: 8,
                touched: true,
            },
            Span {
                start: 8,
                end: 16,
                touched: false,
            },
        ]);
        assert_eq!(cover.len(), 2);
        assert!(cover[0].touched);
        assert!(!cover[1].touched);
    }
}
