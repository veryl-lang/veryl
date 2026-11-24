//! Dirty-tracked cells of a hierarchy template.
//!
//! A template's statements are grouped into the dataflow plan's cells (each
//! comb byte has one writing cell), split along the template's classes.  Every
//! cell is compiled once and run per instance at the instance's delta, only
//! when something it reads changed since its last run.  Inside the template
//! the writer signals the change: a cell compares the bytes other cells read
//! around its run and marks those readers.  FF state and comb bytes events
//! write are marked from outside through the reader tables, like a dataflow
//! settle's; input ports the parent writes are compared against a copy when
//! the reading class starts, since the parent's code does not mark.
//!
//! Each instance is a partition of the dataflow dirty buffer: its cells' dirty
//! bits, then the input copies.

use crate::backend::registry::compile_chunks_parallel;
use crate::backend::{ChunkArtifact, CompileCtx};
use crate::ir::context::Context;
use crate::ir::deps::{StmtDeps, collect_stmt_deps};
use crate::ir::statement::ProtoStatement;
use crate::ir::variable::value_size;
use crate::{HashMap, HashSet};
use std::sync::Arc;

struct SubCell {
    artifact: Arc<ChunkArtifact>,
    /// The cell compiled to report whether its boundary changed, in place of
    /// the compare around `artifact`.
    report: Option<crate::ReportFuncPtr>,
    /// Later cells writing bytes this one writes, and cells reading FF bytes
    /// it writes: they rerun whenever it does.
    cowriters: Vec<u32>,
    /// Comb bytes other cells read, as `(start, len)` in template coordinates.
    boundary: Vec<(isize, u32)>,
    consumers: Vec<u32>,
    /// Per `boundary` range, the cells reading it.
    range_consumers: Vec<Vec<u32>>,
    /// Writes FF storage or has a side effect: runs at every settle.
    always: bool,
    /// Byte offset of this cell's boundary in the shadow the AOT-C cells keep.
    shadow_at: u32,
}

/// Comb bytes the cells read and nothing marks: the parent's writes.
struct ExtInput {
    start: isize,
    len: u32,
    /// Byte offset of this input's copy past the partition's dirty bits.
    copy: u32,
    readers: Vec<u32>,
}

pub(crate) struct HierCells {
    subcells: Vec<SubCell>,
    /// Per class: its cells, a contiguous range.
    class_range: Vec<(u32, u32)>,
    /// Per class: the compared inputs its cells read.
    class_ext: Vec<Vec<u32>>,
    ext: Vec<ExtInput>,
    /// Per class: the cell of each statement, in the class block's order.
    class_stmt_cell: Vec<Vec<u32>>,
    bit_words: usize,
    /// The `always` cells, as bits laid out like a partition's dirty bits.
    always_bits: Vec<u64>,
    /// Partition length in dirty words: the bits, then the input copies.
    pub(crate) words: usize,
    /// Comb bytes read from outside the cells (plus the bytes events also
    /// write, with their writing cells), in template coordinates.
    pub(crate) comb_marked: Arc<crate::ir::opt::dataflow::Readers>,
    /// FF bytes the cells read.
    pub(crate) ff_marked: Arc<crate::ir::opt::dataflow::Readers>,
    pub(crate) fingerprint: u64,
    /// The cells as AOT-C, once compiled (see `emit_hier_template`).
    #[cfg(not(target_family = "wasm"))]
    aot: Option<crate::backend::aot_c::emit::AotCell>,
    /// Dirty word where the AOT-C cells' boundary shadow starts.
    shadow_word: usize,
}

/// Bytes two ranges may sit apart and still be compared as one: a few extra
/// bytes cost less than another comparison, and only ever add a mark.
const GAP: isize = 32;

type Reported = (crate::ReportFuncPtr, Arc<ChunkArtifact>);

/// The code of a cell whose template runs through its AOT-C module.
unsafe extern "system" fn aot_only_cell(_: *const u8, _: *const u8, _: *mut u8, _: isize) {
    unreachable!("a template cell ran outside its AOT-C module")
}

#[cfg(target_family = "wasm")]
fn compile_reporting(
    _config: &crate::ir::Config,
    owned: &[Vec<ProtoStatement>],
    _boundary: &[Vec<(isize, isize)>],
) -> Vec<Option<Reported>> {
    vec![None; owned.len()]
}

/// The cells whose boundary is small enough to compare inside the compiled
/// code, compiled that way in parallel.
#[cfg(not(target_family = "wasm"))]
fn compile_reporting(
    config: &crate::ir::Config,
    owned: &[Vec<ProtoStatement>],
    boundary: &[Vec<(isize, isize)>],
) -> Vec<Option<Reported>> {
    let one = |i: usize| -> Option<Reported> {
        let mut b = boundary[i].clone();
        merge(&mut b, 0);
        if b.iter().any(|&(s, e)| s < 0 || e > i32::MAX as isize) {
            return None;
        }
        let b: Vec<(u32, u32)> = b.iter().map(|&(s, e)| (s as u32, e as u32)).collect();
        let (f, keep) = crate::backend::cranelift::runtime::build_reporting_binary(
            config,
            owned[i].clone(),
            &b,
        )?;
        // SAFETY: the extra return value is ignored by a `FuncPtr` caller.
        let func = unsafe { std::mem::transmute::<crate::ReportFuncPtr, crate::FuncPtr>(f) };
        let artifact = Arc::new(ChunkArtifact {
            func,
            keepalive: keep.map(|m| Box::new(m) as Box<dyn Send + Sync>),
            content_fp: None,
        });
        Some((f, artifact))
    };
    let n = owned.len();
    let threads = std::thread::available_parallelism()
        .map_or(1, |p| p.get())
        .clamp(1, 16);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut out: Vec<Option<Reported>> = vec![None; n];
    let parts: Vec<Vec<(usize, Option<Reported>)>> = std::thread::scope(|sc| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                std::thread::Builder::new()
                    .stack_size(crate::IR_WALK_STACK_BYTES)
                    .spawn_scoped(sc, || {
                        let mut mine = Vec::new();
                        loop {
                            let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            if i >= n {
                                break;
                            }
                            mine.push((i, one(i)));
                        }
                        mine
                    })
                    .expect("spawn cell compiler")
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for (i, r) in parts.into_iter().flatten() {
        out[i] = r;
    }
    out
}

/// Merge `(start, end)` intervals in place, bridging gaps up to `gap`.
fn merge(v: &mut Vec<(isize, isize)>, gap: isize) {
    v.sort_unstable();
    let mut out: Vec<(isize, isize)> = Vec::with_capacity(v.len());
    for &(s, e) in v.iter() {
        match out.last_mut() {
            Some(last) if s <= last.1 + gap => last.1 = last.1.max(e),
            _ => out.push((s, e)),
        }
    }
    *v = out;
}

/// A cell's boundary as compared ranges, each with the cells reading it:
/// ranges overlapping, or near enough and read by the same cells, compare as
/// one.
fn boundary_ranges(mut raw: Vec<(isize, isize, u32)>) -> (Vec<(isize, isize)>, Vec<Vec<u32>>) {
    raw.sort_unstable();
    let mut spans: Vec<(isize, isize, Vec<u32>)> = Vec::new();
    for (s, e, c) in raw {
        match spans.last_mut() {
            Some(last) if last.0 == s && last.1 == e => last.2.push(c),
            _ => spans.push((s, e, vec![c])),
        }
    }
    let mut out: Vec<(isize, isize, Vec<u32>)> = Vec::with_capacity(spans.len());
    for (s, e, mut c) in spans {
        c.sort_unstable();
        c.dedup();
        match out.last_mut() {
            Some(last) if s < last.1 || (s <= last.1 + GAP && last.2 == c) => {
                last.1 = last.1.max(e);
                if last.2 != c {
                    last.2.extend(c);
                    last.2.sort_unstable();
                    last.2.dedup();
                }
            }
            _ => out.push((s, e, c)),
        }
    }
    let ranges = out.iter().map(|r| (r.0, r.1)).collect();
    (ranges, out.into_iter().map(|r| r.2).collect())
}

/// `(ff, start, end)` of one read or written variable.
type Span = (bool, isize, isize);

fn deps_of(s: &ProtoStatement, use_4state: bool) -> (Vec<Span>, Vec<Span>) {
    let mut d = StmtDeps::default();
    collect_stmt_deps(s, &mut d);
    let span = |dep: &crate::ir::deps::Dep| {
        let nb = dep.bytes.map(|b| b as usize).unwrap_or(8);
        let len = value_size(nb, use_4state) as isize;
        (dep.off.is_ff(), dep.off.raw(), dep.off.raw() + len)
    };
    (
        d.ins.iter().map(span).collect(),
        d.outs.iter().map(span).collect(),
    )
}

/// `(ff, start, end, cell)` folded into disjoint comb and FF reader tables.
fn reader_tables(
    mut raw: Vec<(bool, isize, isize, u32)>,
) -> Option<(
    Arc<crate::ir::opt::dataflow::Readers>,
    Arc<crate::ir::opt::dataflow::Readers>,
)> {
    raw.sort_unstable();
    raw.dedup();
    let mut tables: [crate::ir::opt::dataflow::Readers; 2] = Default::default();
    for (ff, s0, e0, cell) in raw {
        let (Ok(a), Ok(b)) = (u32::try_from(s0), u32::try_from(e0)) else {
            return decline(format_args!("a read at a negative offset"));
        };
        let table = &mut tables[ff as usize];
        match table.last_mut() {
            Some(last) if last.0 == a && last.1 == b => last.2.push(cell),
            _ => table.push((a, b, vec![cell])),
        }
    }
    let [comb, ff] = tables.map(|t| Arc::new(crate::ir::opt::dataflow::merge_overlaps(t)));
    Some((comb, ff))
}

fn decline<T>(why: std::fmt::Arguments) -> Option<T> {
    if crate::ir::hier::diag() {
        eprintln!("[hier] cells declined: {why}");
    }
    None
}

/// Build the cells of a template for one class partition, or `None` when a
/// cell does not compile or the plan's single-writer property does not hold
/// for these statements.
pub(crate) fn build(
    context: &mut Context,
    stmts: &[ProtoStatement],
    plan_cells: &[Vec<u32>],
    class_of: &[u32],
    class_order: &[u32],
    event_written: &HashSet<isize>,
) -> Option<Arc<HierCells>> {
    let use_4state = context.config.use_4state;
    let classes = class_order.len();
    let mut rank = vec![0u32; classes];
    for (r, &c) in class_order.iter().enumerate() {
        rank[c as usize] = r as u32;
    }
    // Split each plan cell by class; order cells by class rank, then by plan
    // order, which is topological.
    let mut parts: Vec<(u32, u32, Vec<u32>)> = Vec::new();
    for (ci, cell) in plan_cells.iter().enumerate() {
        let mut by_class: HashMap<u32, Vec<u32>> = HashMap::default();
        for &s in cell {
            by_class.entry(class_of[s as usize]).or_default().push(s);
        }
        for (c, mut v) in by_class {
            v.sort_unstable();
            parts.push((rank[c as usize], ci as u32, v));
        }
    }
    parts.sort_by_key(|p| (p.0, p.1));
    let class_range: Vec<(u32, u32)> = (0..classes)
        .map(|c| {
            let r = rank[c];
            (
                parts.partition_point(|p| p.0 < r) as u32,
                parts.partition_point(|p| p.0 <= r) as u32,
            )
        })
        .collect();

    let n = parts.len();
    let mut reads: Vec<Vec<(bool, isize, isize)>> = Vec::with_capacity(n);
    let mut writes: Vec<Vec<(isize, isize)>> = Vec::with_capacity(n);
    let mut always = vec![false; n];
    // `(cell, start, end)` of every FF write, in template coordinates.
    let mut ff_writes: Vec<(u32, isize, isize)> = Vec::new();
    for (i, (_, _, ss)) in parts.iter().enumerate() {
        let mut r = Vec::new();
        let mut w = Vec::new();
        for &s in ss {
            let stmt = &stmts[s as usize];
            if crate::ir::opt::owners::has_side_effects(stmt) {
                always[i] = true;
            }
            let (ins, outs) = deps_of(stmt, use_4state);
            r.extend(ins);
            for (ff, s0, e0) in outs {
                if ff {
                    always[i] = true;
                    ff_writes.push((i as u32, s0, e0));
                } else {
                    w.push((s0, e0));
                }
            }
        }
        merge(&mut w, 0);
        reads.push(r);
        writes.push(w);
    }

    // Disjoint pieces of the written comb bytes, each with its writing
    // cells in run order.
    let mut events: Vec<(isize, bool, u32)> = Vec::new();
    for (i, w) in writes.iter().enumerate() {
        for &(s0, e0) in w {
            events.push((s0, true, i as u32));
            events.push((e0, false, i as u32));
        }
    }
    events.sort_unstable_by_key(|&(p, open, c)| (p, open, c));
    let mut pieces: Vec<(isize, isize, Vec<u32>)> = Vec::new();
    let mut active: Vec<u32> = Vec::new();
    let mut k = 0;
    while k < events.len() {
        let at = events[k].0;
        while k < events.len() && events[k].0 == at {
            let (_, open, c) = events[k];
            if open {
                active.push(c);
            } else if let Some(j) = active.iter().position(|&x| x == c) {
                active.swap_remove(j);
            }
            k += 1;
        }
        if let Some(&(next, _, _)) = events.get(k)
            && !active.is_empty()
        {
            let mut owners = active.clone();
            owners.sort_unstable();
            owners.dedup();
            pieces.push((at, next, owners));
        }
    }
    // A later writer of a byte an earlier one rewrites must run after it.
    let mut cowriters: Vec<Vec<u32>> = vec![Vec::new(); n];
    for (_, _, owners) in &pieces {
        for w in owners.windows(2) {
            cowriters[w[0] as usize].push(w[1]);
        }
    }
    // Writers of one byte in different cells may sit on either side of a
    // reader of an intermediate value; skipping one of them would hand that
    // reader the wrong value, so they all run.
    for i in 0..n {
        if !cowriters[i].is_empty() {
            always[i] = true;
            for &c in &cowriters[i] {
                always[c as usize] = true;
            }
        }
    }
    let mut evt: Vec<isize> = event_written.iter().copied().collect();
    evt.sort_unstable();

    let mut boundary: Vec<Vec<(isize, isize)>> = vec![Vec::new(); n];
    let mut boundary_readers: Vec<Vec<(isize, isize, u32)>> = vec![Vec::new(); n];
    let mut consumers: Vec<Vec<u32>> = vec![Vec::new(); n];
    // `(ff, start, end, cell)` an outside write marks.
    let mut marked_raw: Vec<(bool, isize, isize, u32)> = Vec::new();
    // `(start, end, cell)` the parent writes, compared instead.
    let mut ext_raw: Vec<(isize, isize, u32)> = Vec::new();
    // A comb byte an event also writes: its writing cells rerun to restore
    // their value once the event changed it.
    for (s0, e0, owners) in &pieces {
        let k = evt.partition_point(|o| o < s0);
        if evt.get(k).is_some_and(|o| o < e0) {
            for &w in owners {
                marked_raw.push((false, *s0, *e0, w));
            }
        }
    }
    for (i, r) in reads.iter().enumerate() {
        for &(ff, s0, e0) in r {
            if ff {
                marked_raw.push((true, s0, e0, i as u32));
                for &(w, ws, we) in &ff_writes {
                    if w as usize != i && ws < e0 && s0 < we {
                        cowriters[w as usize].push(i as u32);
                    }
                }
                continue;
            }
            let mut at = s0;
            let mut k = pieces.partition_point(|o| o.1 <= s0);
            while at < e0 {
                match pieces.get(k) {
                    Some((ws, we, owners)) if *ws < e0 => {
                        if *ws > at {
                            ext_raw.push((at, *ws, i as u32));
                        }
                        let (os, oe) = ((*ws).max(s0), (*we).min(e0));
                        for &p in owners {
                            if p as usize != i {
                                boundary[p as usize].push((os, oe));
                                boundary_readers[p as usize].push((os, oe, i as u32));
                                consumers[p as usize].push(i as u32);
                            }
                        }
                        at = (*we).max(at);
                        k += 1;
                    }
                    _ => {
                        ext_raw.push((at, e0, i as u32));
                        at = e0;
                    }
                }
            }
        }
    }

    marked_raw.extend(ext_raw.iter().map(|&(s0, e0, c)| (false, s0, e0, c)));
    let (comb_marked, ff_marked) = reader_tables(marked_raw)?;
    ext_raw.sort_unstable();
    let mut ext: Vec<ExtInput> = Vec::new();
    for (s0, e0, reader) in ext_raw {
        match ext.last_mut() {
            Some(x) if s0 <= x.start + x.len as isize + GAP => {
                let end = (x.start + x.len as isize).max(e0);
                x.len = (end - x.start) as u32;
                x.readers.push(reader);
            }
            _ => ext.push(ExtInput {
                start: s0,
                len: (e0 - s0) as u32,
                copy: 0,
                readers: vec![reader],
            }),
        }
    }
    let mut at = 0u32;
    for x in &mut ext {
        x.copy = at;
        at += x.len;
        x.readers.sort_unstable();
        x.readers.dedup();
    }
    let mut class_ext: Vec<Vec<u32>> = vec![Vec::new(); classes];
    for (e, x) in ext.iter().enumerate() {
        let mut seen: HashSet<u32> = HashSet::default();
        for &r in &x.readers {
            let c = class_order[parts[r as usize].0 as usize];
            if seen.insert(c) {
                class_ext[c as usize].push(e as u32);
            }
        }
    }

    let owned: Vec<Vec<ProtoStatement>> = parts
        .iter()
        .map(|(_, _, ss)| ss.iter().map(|&s| stmts[s as usize].clone()).collect())
        .collect();
    let stub = Arc::new(ChunkArtifact {
        func: aot_only_cell,
        keepalive: None,
        content_fp: None,
    });
    let mut subcells = Vec::with_capacity(n);
    for i in 0..n {
        let (b, range_consumers) = boundary_ranges(std::mem::take(&mut boundary_readers[i]));
        let mut c = std::mem::take(&mut consumers[i]);
        c.sort_unstable();
        c.dedup();
        let mut cw = std::mem::take(&mut cowriters[i]);
        cw.sort_unstable();
        cw.dedup();
        subcells.push(SubCell {
            artifact: Arc::clone(&stub),
            report: None,
            cowriters: cw,
            boundary: b.into_iter().map(|(s, e)| (s, (e - s) as u32)).collect(),
            range_consumers,
            consumers: c,
            always: always[i],
            shadow_at: 0,
        });
    }
    let mut cell_of = vec![0u32; stmts.len()];
    for (k, (_, _, ss)) in parts.iter().enumerate() {
        for &s in ss {
            cell_of[s as usize] = k as u32;
        }
    }
    let mut class_stmt_cell: Vec<Vec<u32>> = vec![Vec::new(); classes];
    for (s, &c) in class_of.iter().enumerate() {
        class_stmt_cell[c as usize].push(cell_of[s]);
    }
    let bit_words = n.div_ceil(64);
    let mut always_bits = vec![0u64; bit_words];
    for (k, sc) in subcells.iter().enumerate() {
        if sc.always {
            always_bits[k / 64] |= 1 << (k % 64);
        }
    }
    let shadow_word = bit_words + (at as usize).div_ceil(8);
    let mut shadow_bytes = 0u32;
    for sc in &mut subcells {
        sc.shadow_at = shadow_bytes;
        shadow_bytes += sc.boundary.iter().map(|&(_, len)| len).sum::<u32>();
    }
    #[cfg(not(target_family = "wasm"))]
    let aot = if context.config.aot_c && !use_4state {
        compile_template(
            &context.config,
            &owned,
            &subcells,
            &class_range,
            &always_bits,
            shadow_word,
        )
    } else {
        None
    };
    // A module already in the cache runs every class from the start, so the
    // cells need no code of their own.
    #[cfg(not(target_family = "wasm"))]
    let aot_loaded = aot.as_ref().is_some_and(|c| c.get().is_some());
    #[cfg(target_family = "wasm")]
    let aot_loaded = false;
    if !aot_loaded {
        let reports = if use_4state {
            vec![None; n]
        } else {
            compile_reporting(&context.config, &owned, &boundary)
        };
        let rest: Vec<usize> = (0..n).filter(|&i| reports[i].is_none()).collect();
        let groups: Vec<&[ProtoStatement]> = rest.iter().map(|&i| owned[i].as_slice()).collect();
        let compiled = {
            let ctx = CompileCtx {
                config: &context.config,
                use_4state,
                contains_compiled_block: false,
                df_marks: None,
            };
            compile_chunks_parallel(&mut context.backends, &ctx, &groups)
        };
        for (i, r) in reports.into_iter().enumerate() {
            if let Some((f, artifact)) = r {
                subcells[i].report = Some(f);
                subcells[i].artifact = artifact;
            }
        }
        for (&i, a) in rest.iter().zip(compiled) {
            let Some(a) = a else {
                return decline(format_args!(
                    "cell {i} of {} statements did not compile",
                    owned[i].len()
                ));
            };
            subcells[i].artifact = a;
        }
        if crate::ir::hier::diag() {
            eprintln!(
                "[hier] cells: {n}, {} report their boundary change",
                n - rest.len()
            );
        }
    }
    use std::hash::{Hash, Hasher};
    // The statements too: two templates of one shape are told apart by it,
    // their instances' class runs keyed on it.
    let mut h = std::collections::hash_map::DefaultHasher::new();
    stmts.hash(&mut h);
    plan_cells.hash(&mut h);
    class_of.hash(&mut h);
    class_order.hash(&mut h);
    Some(Arc::new(HierCells {
        subcells,
        class_range,
        class_ext,
        ext,
        class_stmt_cell,
        bit_words,
        always_bits,
        words: shadow_word + (shadow_bytes as usize).div_ceil(8),
        comb_marked,
        ff_marked,
        fingerprint: h.finish(),
        #[cfg(not(target_family = "wasm"))]
        aot,
        shadow_word,
    }))
}

/// Bits of `cells` as the `(word, mask)` pairs the emitted code ORs in.
#[cfg(not(target_family = "wasm"))]
fn word_masks(cells: &[u32]) -> Vec<(u32, u64)> {
    let mut out: Vec<(u32, u64)> = Vec::new();
    for &c in cells {
        let (w, m) = (c / 64, 1u64 << (c % 64));
        match out.iter_mut().find(|x| x.0 == w) {
            Some(x) => x.1 |= m,
            None => out.push((w, m)),
        }
    }
    out
}

/// The cells as one AOT-C module, compiled in the background.
#[cfg(not(target_family = "wasm"))]
fn compile_template(
    config: &crate::ir::Config,
    owned: &[Vec<ProtoStatement>],
    subcells: &[SubCell],
    class_range: &[(u32, u32)],
    always_bits: &[u64],
    shadow_word: usize,
) -> Option<crate::backend::aot_c::emit::AotCell> {
    use crate::backend::aot_c::emit::{TemplateCell, prepare_hier_template};
    let mut cells = Vec::with_capacity(subcells.len());
    for (sc, stmts) in subcells.iter().zip(owned) {
        let mut boundary = Vec::with_capacity(sc.boundary.len());
        for &(s, len) in &sc.boundary {
            let s = u32::try_from(s).ok()?;
            boundary.push((s, s.checked_add(len)?));
        }
        cells.push(TemplateCell {
            stmts: stmts.iter().collect(),
            boundary,
            on_change: sc.range_consumers.iter().map(|c| word_masks(c)).collect(),
            on_run: word_masks(&sc.cowriters),
        });
    }
    let cell = prepare_hier_template(
        &cells,
        class_range,
        always_bits,
        shadow_word,
        config.aot_c_async,
    );
    if crate::ir::hier::diag() {
        eprintln!(
            "[hier] AOT-C cells: {}",
            if cell.is_some() {
                "emitted"
            } else {
                "declined"
            }
        );
    }
    cell
}

impl HierCells {
    /// Whether the AOT-C cells have loaded.
    #[cfg(all(test, not(target_family = "wasm")))]
    pub(crate) fn aot_landed(&self) -> bool {
        self.aot.as_ref().is_some_and(|c| c.get().is_some())
    }

    /// A compiled cell of the class, standing in for the class block's
    /// artifact; `None` when the class has no cells.
    pub(crate) fn class_artifact(&self, class: u32) -> Option<Arc<ChunkArtifact>> {
        let (lo, hi) = self.class_range[class as usize];
        (lo < hi).then(|| Arc::clone(&self.subcells[lo as usize].artifact))
    }

    pub(crate) fn cell_count(&self) -> u32 {
        self.subcells.len() as u32
    }

    /// The cell holding statement `index` of `class`'s block.
    pub(crate) fn cell_of(&self, class: u32, index: usize) -> u32 {
        self.class_stmt_cell[class as usize][index]
    }

    pub(crate) fn summary(&self) -> String {
        let ext_bytes: usize = self.ext.iter().map(|x| x.len as usize).sum();
        let always = self.subcells.iter().filter(|s| s.always).count();
        format!(
            "{} cells ({} always), {} compared inputs over {} bytes, {} comb and {} FF marked ranges",
            self.subcells.len(),
            always,
            self.ext.len(),
            ext_bytes,
            self.comb_marked.len(),
            self.ff_marked.len(),
        )
    }
}

/// A class run as the compiled parent names it: the class, the instance's
/// comb delta and the template's fingerprint, since instances of two
/// templates may sit at the same delta.
pub(crate) type RunKey = (u32, isize, u64);

/// One class of one instance, as the settle runs it.
#[derive(Clone)]
pub struct HierClassRun {
    pub(crate) cells: Arc<HierCells>,
    pub(crate) class: u32,
    /// The instance's partition of the dataflow cell layout.
    pub(crate) partition: u32,
    pub(crate) ff: *const u8,
    pub(crate) comb: *const u8,
    /// The instance's partition of the dirty buffer, set by the `Ir`.
    pub(crate) bits: *mut u64,
    pub(crate) log_buf: *mut u8,
    pub(crate) ff_delta: isize,
    /// Whether the class compares the inputs its parent writes; off once
    /// the parent's own cells mark the readers of what they store.
    pub(crate) compare_inputs: bool,
    /// Whether this instance's shadow of the class's AOT-C cells holds their
    /// boundary values.
    pub(crate) primed: std::cell::Cell<bool>,
}

/// `a[..n] == b[..n]`, without a call for the short ranges cells mostly have.
///
/// # Safety
/// Both pointers address `n` readable bytes.
#[inline(always)]
unsafe fn same(a: *const u8, b: *const u8, n: usize) -> bool {
    unsafe {
        let mut i = 0;
        while i + 8 <= n {
            if (a.add(i) as *const u64).read_unaligned()
                != (b.add(i) as *const u64).read_unaligned()
            {
                return false;
            }
            i += 8;
        }
        while i < n {
            if *a.add(i) != *b.add(i) {
                return false;
            }
            i += 1;
        }
        true
    }
}

thread_local! {
    static SCRATCH: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
    /// Run every cell regardless of its clean bit (the check's rerun).
    pub(crate) static FORCE_ALL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// `VERYL_HIER_CELLS_CHECK=1`: rerun every cell after each settle and fail on
/// any difference.
pub(crate) fn check_enabled() -> bool {
    static ON: std::sync::LazyLock<bool> =
        std::sync::LazyLock::new(|| std::env::var("VERYL_HIER_CELLS_CHECK").is_ok());
    *ON
}

impl HierClassRun {
    /// # Safety
    /// The pointers address this instance's live buffers.
    pub(crate) unsafe fn run(&self) {
        let cells = &*self.cells;
        let bits = self.bits;
        let copies = unsafe { bits.add(cells.bit_words) as *mut u8 };
        let mark = |k: u32| unsafe {
            *bits.add(k as usize / 64) |= 1u64 << (k % 64);
        };
        let compared: &[u32] = if self.compare_inputs {
            &cells.class_ext[self.class as usize]
        } else {
            &[]
        };
        for &e in compared {
            let x = &cells.ext[e as usize];
            unsafe {
                let src = self.comb.offset(x.start);
                let dst = copies.add(x.copy as usize);
                if !same(src, dst, x.len as usize) {
                    std::ptr::copy_nonoverlapping(src, dst, x.len as usize);
                    for &r in &x.readers {
                        mark(r);
                    }
                }
            }
        }
        let (lo, hi) = cells.class_range[self.class as usize];
        let force = FORCE_ALL.with(|f| f.get());
        #[cfg(not(target_family = "wasm"))]
        if let Some(m) = cells.aot.as_ref().and_then(|c| c.get()) {
            let shadow = unsafe { bits.add(cells.shadow_word) as *mut u8 };
            if !self.primed.get() {
                // The boundary bytes hold what each cell last wrote, or what
                // an event stored after it and marked the readers of.
                for sc in &cells.subcells[lo as usize..hi as usize] {
                    let mut at = sc.shadow_at as usize;
                    for &(s, len) in &sc.boundary {
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                self.comb.offset(s),
                                shadow.add(at),
                                len as usize,
                            )
                        };
                        at += len as usize;
                    }
                }
                self.primed.set(true);
            }
            let mode = ((self.class as usize) << 1) | force as usize;
            unsafe {
                (m.func)(self.ff, self.comb, mode as *mut u8, bits as isize);
            }
            return;
        }
        SCRATCH.with(|scratch| {
            let mut scratch = scratch.borrow_mut();
            if lo == hi {
                return;
            }
            for w in lo as usize / 64..=(hi as usize - 1) / 64 {
                let first = (w * 64).max(lo as usize) - w * 64;
                let last = ((w + 1) * 64).min(hi as usize) - w * 64;
                let in_class = (u64::MAX >> (64 - (last - first))) << first;
                let pending_always = if force {
                    u64::MAX
                } else {
                    cells.always_bits[w]
                };
                // Cells left to visit in this word, in order: a cell marking
                // an earlier one leaves it for the next run.
                let mut ahead = in_class;
                loop {
                    // SAFETY: `w` is inside the partition's bit words.
                    let word = unsafe { &mut *bits.add(w) };
                    let pending = (*word | pending_always) & ahead;
                    if pending == 0 {
                        break;
                    }
                    let bit = pending.isolate_lowest_one();
                    ahead &= !(bit | (bit - 1));
                    *word &= !bit;
                    let k = w * 64 + bit.trailing_zeros() as usize;
                    let sc = &cells.subcells[k];
                    if let Some(f) = sc.report {
                        if unsafe { f(self.ff, self.comb, self.log_buf, self.ff_delta) } != 0 {
                            for &c in &sc.consumers {
                                mark(c);
                            }
                        }
                        for &c in &sc.cowriters {
                            mark(c);
                        }
                        continue;
                    }
                    scratch.clear();
                    for &(s, len) in &sc.boundary {
                        scratch.extend_from_slice(unsafe {
                            std::slice::from_raw_parts(self.comb.offset(s), len as usize)
                        });
                    }
                    unsafe {
                        (sc.artifact.func)(self.ff, self.comb, self.log_buf, self.ff_delta);
                    }
                    let mut pos = 0usize;
                    let mut changed = false;
                    for &(s, len) in &sc.boundary {
                        if !unsafe {
                            same(self.comb.offset(s), scratch.as_ptr().add(pos), len as usize)
                        } {
                            changed = true;
                            break;
                        }
                        pos += len as usize;
                    }
                    if changed {
                        for &c in &sc.consumers {
                            mark(c);
                        }
                    }
                    for &c in &sc.cowriters {
                        mark(c);
                    }
                }
            }
        });
    }
}

thread_local! {
    /// The class runs of the `Ir` whose dataflow cells are running, by
    /// `(class, instance comb delta)`.
    static ACTIVE: std::cell::Cell<*const HashMap<RunKey, HierClassRun>> =
        const { std::cell::Cell::new(std::ptr::null()) };
}

/// Make `runs` the class runs compiled cells reach through
/// `veryl_aot_hier_run` until the next call.
pub(crate) fn set_active(runs: *const HashMap<RunKey, HierClassRun>) {
    ACTIVE.with(|a| a.set(runs));
}

/// Callback a compiled dataflow cell makes to run one class of one instance.
///
/// # Safety
/// The runs `set_active` installed must be alive and hold the class.
pub unsafe extern "C" fn veryl_aot_hier_run(class: u32, comb_delta: i64, template: u64) {
    let runs = ACTIVE.with(|a| a.get());
    debug_assert!(!runs.is_null(), "a hierarchy class ran outside a settle");
    let runs = unsafe { &*runs };
    let run = &runs[&(class, comb_delta as isize, template)];
    unsafe { run.run() };
}

/// The partition of each `(cells, instance comb delta)`.
pub(crate) type PartitionIndex = HashMap<(usize, isize), u32>;

/// Number every instance's class blocks with its partition, in first-run
/// order: each partition's `(cells, ff delta, comb delta)`, and the index.
pub(crate) fn assign_partitions(
    stmts: &mut crate::ir::statement::ProtoStatements,
) -> (Vec<(Arc<HierCells>, isize, isize)>, PartitionIndex) {
    let mut index: PartitionIndex = HashMap::default();
    let mut out = Vec::new();
    for block in &mut stmts.0 {
        let crate::ir::statement::ProtoStatementBlock::Interpreted(v) = block else {
            continue;
        };
        for s in v {
            let ProtoStatement::CompiledBlock(cb) = s else {
                continue;
            };
            let (ff_delta, comb_delta) = (cb.ff_delta_bytes, cb.comb_delta_bytes);
            let Some(h) = cb.hier.as_mut() else {
                continue;
            };
            let key = (Arc::as_ptr(&h.cells) as usize, comb_delta);
            h.partition = *index.entry(key).or_insert_with(|| {
                out.push((Arc::clone(&h.cells), ff_delta, comb_delta));
                out.len() as u32 - 1
            });
        }
    }
    (out, index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_boundary_range_marks_only_the_cells_reading_it() {
        // Cell 7 reads [0, 4), cell 8 reads [100, 104) and [120, 124), cell 9
        // reads [2, 6) overlapping cell 7's.
        let raw = vec![(0, 4, 7), (100, 104, 8), (120, 124, 8), (2, 6, 9)];
        let (ranges, readers) = boundary_ranges(raw);
        assert_eq!(ranges, vec![(0, 6), (100, 124)]);
        assert_eq!(readers, vec![vec![7, 9], vec![8]]);
        // Near ranges read by different cells stay apart.
        let (ranges, readers) = boundary_ranges(vec![(0, 4, 1), (8, 12, 2)]);
        assert_eq!(ranges, vec![(0, 4), (8, 12)]);
        assert_eq!(readers, vec![vec![1], vec![2]]);
    }

    #[test]
    fn a_write_inside_a_wide_read_does_not_mark_the_other_element_readers() {
        // Cell 0 reads a whole array, cells 1 and 2 one element each.
        let raw = vec![(false, 0, 16, 0), (false, 0, 4, 1), (false, 8, 12, 2)];
        let (comb, ff) = reader_tables(raw).unwrap();
        assert!(ff.is_empty());
        let readers_of = |at: u32| -> Vec<u32> {
            comb.iter()
                .filter(|r| r.0 <= at && at < r.1)
                .flat_map(|r| r.2.clone())
                .collect()
        };
        assert_eq!(readers_of(1), vec![0, 1]);
        assert_eq!(readers_of(9), vec![0, 2]);
        assert_eq!(readers_of(13), vec![0]);
    }
}
