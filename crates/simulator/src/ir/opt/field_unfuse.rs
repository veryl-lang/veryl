//! Split a packed comb variable whose accesses are all static bit-fields into
//! per-field storage words.
//!
//! A tree-structured net like an arbiter's `index_nodes` is declared as one
//! packed vector but only ever accessed one element at a time with constant
//! indices.  Keeping it packed prices every write as a read-modify-write with
//! a shift/mask pair and every read as a shift/mask extract.  Giving each
//! field its own word turns those into plain stores and loads.
//!
//! Candidacy is decided from the statement lists alone: a comb offset
//! qualifies when every write is a top-level static bit-field store, every
//! read is a static select, and nothing outside the rewritten comb list can
//! observe the storage.  The initial block and the event statements are NOT
//! rewritten, so any offset they touch is disqualified — that also covers
//! every signal a testbench reads or drives, because the `Event::Initial`
//! statements are the testbench.  The retired packed word keeps its meta and
//! its (stale) storage; external byte-level machinery is either conservative
//! about unknown bytes (`tb_dirty`) or fed the field spans through
//! `Context::comb_reloc` (cone gating).
//!
//! Reads that span several fields — or bits no field covers, which under
//! zero-initial two-state storage are constant zero — are rebuilt as a
//! concatenation of the per-field words.

use crate::ir::statement::{
    ProtoAssignStatement, ProtoForBound, ProtoForRange, ProtoStatement, ProtoSystemFunctionCall,
};
use crate::ir::variable::{VarOffset, native_bytes, value_size};
use crate::ir::{ProtoExpression, Value};
use crate::{HashMap, HashSet};

pub fn enabled(use_4state: bool) -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    !use_4state
        && !FORCE_DISABLED.load(std::sync::atomic::Ordering::Relaxed)
        && *ON.get_or_init(|| {
            std::env::var("VERYL_FIELD_UNFUSE").as_deref() != Ok("0")
                // The step watcher reads arbitrary variables through their
                // meta pointers, which a split leaves stale.
                && std::env::var("VERYL_STEP_WATCH").is_err()
        })
}

static FORCE_DISABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Wave dumps read every variable's storage through its meta pointer, which a
/// split leaves stale; called by `veryl test --wave` alongside the localize /
/// fusion kill switches.
pub fn force_disable() {
    FORCE_DISABLED.store(true, std::sync::atomic::Ordering::Relaxed);
}

fn diag() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("VERYL_FIELD_UNFUSE_DIAG").as_deref() == Ok("1"))
}

/// Most fields a single spanning read may gather; past this the concat costs
/// more than the split saves and the variable is left packed.
fn gather_limit() -> usize {
    static V: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("VERYL_FIELD_UNFUSE_GATHER")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(4)
    })
}

#[derive(Default, Debug)]
pub struct RunStats {
    pub vars_split: usize,
    pub fields: usize,
    pub writes_rewritten: usize,
    pub reads_rewritten: usize,
    pub gathers: usize,
    pub skip_disq: usize,
    pub skip_few_fields: usize,
    pub skip_wide_field: usize,
    pub skip_whole_write: usize,
    pub skip_gather: usize,
    pub skip_blocklist: usize,
}

/// Per-offset access census.  Ranges are `(msb, lsb)` absolute bit indices,
/// matching statement/expression `select`.
#[derive(Default)]
struct VarInfo {
    full_width: usize,
    width_conflict: bool,
    write_ranges: Vec<(usize, usize)>,
    read_ranges: Vec<(usize, usize)>,
    whole_write: bool,
    disqualified: bool,
}

#[derive(Default)]
struct Census {
    vars: HashMap<isize, VarInfo>,
    /// Comb byte ranges a runtime-indexed access may touch.
    dyn_spans: Vec<(isize, isize)>,
    /// A statement kind the census cannot bound was seen; the pass bails.
    bail: bool,
}

impl Census {
    fn var(&mut self, off: isize) -> &mut VarInfo {
        self.vars.entry(off).or_default()
    }
    fn disq(&mut self, off: isize) {
        self.var(off).disqualified = true;
    }
    fn note_width(&mut self, off: isize, w: usize) {
        let v = self.var(off);
        if v.full_width == 0 {
            v.full_width = w;
        } else if v.full_width != w {
            v.width_conflict = true;
        }
    }
    fn write(&mut self, off: isize, w: usize, sel: Option<(usize, usize)>, poison: bool) {
        if poison {
            self.disq(off);
            return;
        }
        self.note_width(off, w);
        let v = self.var(off);
        match sel {
            Some((msb, lsb)) if msb >= lsb && msb < w => v.write_ranges.push((msb, lsb)),
            Some(_) => v.disqualified = true,
            None => v.whole_write = true,
        }
    }
    fn read(&mut self, off: isize, w: usize, sel: Option<(usize, usize)>, poison: bool) {
        if poison {
            self.disq(off);
            return;
        }
        self.note_width(off, w);
        let v = self.var(off);
        match sel {
            Some((msb, lsb)) if msb >= lsb && msb < w => v.read_ranges.push((msb, lsb)),
            Some(_) => v.disqualified = true,
            None if w > 0 => v.read_ranges.push((w - 1, 0)),
            None => v.disqualified = true,
        }
    }
}

fn census_expr(e: &ProtoExpression, c: &mut Census, poison: bool) {
    match e {
        ProtoExpression::Variable {
            var_offset,
            select,
            dynamic_select,
            var_full_width,
            ..
        } => {
            if let VarOffset::Comb(o) = var_offset {
                if dynamic_select.is_some() {
                    c.disq(*o);
                } else {
                    c.read(*o, *var_full_width, *select, poison);
                }
            }
            if let Some(ds) = dynamic_select {
                census_expr(&ds.index_expr, c, poison);
            }
        }
        ProtoExpression::Value { .. } => {}
        ProtoExpression::Unary { x, .. } => census_expr(x, c, poison),
        ProtoExpression::Binary { x, y, .. } => {
            census_expr(x, c, poison);
            census_expr(y, c, poison);
        }
        ProtoExpression::Concatenation { elements, .. } => {
            for (x, _, _) in elements {
                census_expr(x, c, poison);
            }
        }
        ProtoExpression::Ternary {
            cond,
            true_expr,
            false_expr,
            ..
        } => {
            census_expr(cond, c, poison);
            census_expr(true_expr, c, poison);
            census_expr(false_expr, c, poison);
        }
        ProtoExpression::DynamicVariable {
            base_offset,
            stride,
            element_native_bytes,
            index_expr,
            num_elements,
            dynamic_select,
            ..
        } => {
            if let VarOffset::Comb(o) = base_offset {
                let span =
                    stride.unsigned_abs() * num_elements.saturating_sub(1) + element_native_bytes;
                let base = if *stride < 0 {
                    o + stride * (num_elements.saturating_sub(1) as isize)
                } else {
                    *o
                };
                c.dyn_spans.push((base, base + span as isize));
            }
            census_expr(index_expr, c, poison);
            if let Some(ds) = dynamic_select {
                census_expr(&ds.index_expr, c, poison);
            }
        }
        // Resolved before conv; seeing one means the invariant broke.
        ProtoExpression::HierVariable(_) => c.bail = true,
    }
}

fn census_stmt(s: &ProtoStatement, c: &mut Census, poison: bool) {
    match s {
        ProtoStatement::Assign(a) => {
            if let VarOffset::Comb(o) = a.dst {
                if a.dynamic_select.is_some() {
                    c.disq(o);
                } else {
                    c.write(o, a.dst_width, a.select, poison);
                }
            }
            if let Some(ds) = &a.dynamic_select {
                census_expr(&ds.index_expr, c, poison);
            }
            census_expr(&a.expr, c, poison);
        }
        ProtoStatement::AssignDynamic(a) => {
            if let VarOffset::Comb(o) = a.dst_base {
                let span = a.dst_stride.unsigned_abs() * a.dst_num_elements.saturating_sub(1)
                    + native_bytes(a.dst_width).max(1);
                let base =
                    o.min(o + a.dst_stride * (a.dst_num_elements.saturating_sub(1) as isize));
                c.dyn_spans.push((base, base + span as isize));
            }
            census_expr(&a.dst_index_expr, c, poison);
            census_expr(&a.expr, c, poison);
            if let Some(ds) = &a.dynamic_select {
                census_expr(&ds.index_expr, c, poison);
            }
        }
        ProtoStatement::If(x) => {
            if let Some(cond) = &x.cond {
                census_expr(cond, c, poison);
            }
            for s in x.true_side.iter().chain(x.false_side.iter()) {
                census_stmt(s, c, poison);
            }
        }
        ProtoStatement::Case(x) => {
            for arm in &x.arms {
                census_expr(&arm.cond, c, poison);
                for s in &arm.body {
                    census_stmt(s, c, poison);
                }
            }
            for s in &x.default {
                census_stmt(s, c, poison);
            }
        }
        ProtoStatement::For(x) => {
            if let VarOffset::Comb(o) = x.var_offset {
                c.disq(o);
            }
            let (start, end) = match &x.range {
                ProtoForRange::Forward { start, end, .. }
                | ProtoForRange::Reverse { start, end, .. }
                | ProtoForRange::Stepped { start, end, .. } => (start, end),
            };
            for b in [start, end] {
                if let ProtoForBound::Dynamic(e) = b {
                    census_expr(e, c, poison);
                }
            }
            for s in &x.body {
                census_stmt(s, c, poison);
            }
        }
        ProtoStatement::SequentialBlock(body) => {
            for s in body {
                census_stmt(s, c, poison);
            }
        }
        ProtoStatement::SystemFunctionCall(x) => match x {
            ProtoSystemFunctionCall::Display { args, .. }
            | ProtoSystemFunctionCall::Write { args, .. } => {
                for a in args {
                    census_expr(a, c, poison);
                }
            }
            ProtoSystemFunctionCall::Assert {
                condition, args, ..
            } => {
                census_expr(condition, c, poison);
                for a in args {
                    census_expr(a, c, poison);
                }
            }
            ProtoSystemFunctionCall::Readmemh { elements, .. } => {
                for e in elements {
                    if let VarOffset::Comb(o) = e.current {
                        c.disq(o);
                    }
                }
            }
            ProtoSystemFunctionCall::Finish => {}
        },
        // Executes a pre-compiled artifact with baked offsets; the original
        // statements say what it touches, and none of that may move.
        ProtoStatement::CompiledBlock(x) => {
            for s in &x.original_stmts {
                census_stmt(s, c, true);
            }
        }
        // The ARGUMENT expressions can carry hierarchically-resolved DUT
        // comb reads no blocklist covers, and the interpreter evaluates
        // them with baked offsets — census them like any other read.  The
        // component's own state and connects are on the caller's blocklist;
        // in the comb list the call still reaches state the census cannot
        // bound, so it bails there as before.
        ProtoStatement::TbMethodCall { method, .. } => {
            use crate::ir::statement::{ProtoComponentArg, ProtoTbMethodKind as K};
            match method {
                K::ClockNext { count, period } => {
                    for e in count.iter().chain(period.iter()) {
                        census_expr(e, c, poison);
                    }
                }
                K::ResetAssert { duration, .. } => {
                    if let Some(d) = duration {
                        census_expr(d, c, poison);
                    }
                }
                K::FileWrite { args, .. } => {
                    for a in args {
                        census_expr(a, c, poison);
                    }
                }
                K::Component { args, .. } => {
                    for a in args {
                        if let ProtoComponentArg::Expr(e) = a {
                            census_expr(e, c, poison);
                        }
                    }
                }
                K::RandomSeed { value } => census_expr(value, c, poison),
                K::RandomGetRange { min, max, .. } => {
                    census_expr(min, c, poison);
                    census_expr(max, c, poison);
                }
                K::FileOpen { .. }
                | K::FileClose
                | K::FileFlush
                | K::RandomGet { .. }
                | K::RandomGetSeed { .. } => {}
            }
            if !poison {
                c.bail = true;
            }
        }
        ProtoStatement::Break => {}
    }
}

/// One split variable: its fields sorted by `lsb`, each with its new storage.
struct SplitVar {
    /// `(lsb, msb, new_offset, field_width)`
    fields: Vec<(usize, usize, isize, usize)>,
}

impl SplitVar {
    /// The field containing `[msb:lsb]` entirely, if any.
    fn containing(&self, msb: usize, lsb: usize) -> Option<&(usize, usize, isize, usize)> {
        let i = self.fields.partition_point(|&(lo, _, _, _)| lo <= lsb);
        let f = &self.fields[i.checked_sub(1)?];
        (f.0 <= lsb && msb <= f.1).then_some(f)
    }
    /// Fields overlapping `[msb:lsb]`.
    fn overlapping(&self, msb: usize, lsb: usize) -> &[(usize, usize, isize, usize)] {
        let a = self.fields.partition_point(|&(_, hi, _, _)| hi < lsb);
        let b = self.fields.partition_point(|&(lo, _, _, _)| lo <= msb);
        &self.fields[a..b]
    }
}

/// Rebase an access `[msb:lsb]` (absolute bits, contained in the field) onto
/// the field's own storage: `(offset, field_width, select)`.  One definition
/// serving read and write paths, so the two cannot disagree about where a
/// bit lives.
fn rebase(
    f: &(usize, usize, isize, usize),
    msb: usize,
    lsb: usize,
) -> (isize, usize, Option<(usize, usize)>) {
    let (flo, fhi, off, fw) = *f;
    debug_assert!(flo <= lsb && msb <= fhi);
    let select = if lsb == flo && msb == fhi {
        None
    } else {
        Some((msb - flo, lsb - flo))
    };
    (off, fw, select)
}

fn field_read(
    f: &(usize, usize, isize, usize),
    msb: usize,
    lsb: usize,
    signed: bool,
) -> ProtoExpression {
    let (off, fw, select) = rebase(f, msb, lsb);
    let width = msb - lsb + 1;
    ProtoExpression::Variable {
        var_offset: VarOffset::Comb(off),
        select,
        dynamic_select: None,
        width,
        var_full_width: fw,
        expr_context: crate::ir::expression::ExpressionContext { width, signed },
    }
}

struct Rewriter<'a> {
    map: &'a HashMap<isize, SplitVar>,
    stats: &'a mut RunStats,
}

impl Rewriter<'_> {
    fn expr(&mut self, e: &mut ProtoExpression) {
        match e {
            ProtoExpression::Variable {
                var_offset,
                select,
                dynamic_select,
                width,
                var_full_width,
                expr_context,
            } => {
                debug_assert!(dynamic_select.is_none() || !self.mapped(var_offset));
                if let Some(ds) = dynamic_select {
                    self.expr(&mut ds.index_expr);
                    return;
                }
                let VarOffset::Comb(o) = var_offset else {
                    return;
                };
                let Some(sv) = self.map.get(o) else { return };
                let (msb, lsb) = select.unwrap_or((*var_full_width - 1, 0));
                if let Some(f) = sv.containing(msb, lsb) {
                    let (off, fw, sel) = rebase(f, msb, lsb);
                    *var_offset = VarOffset::Comb(off);
                    *var_full_width = fw;
                    *select = sel;
                    self.stats.reads_rewritten += 1;
                    return;
                }
                // Spanning read: gather covered fields MSB-first; bits no
                // field owns were never written, so with zero-initial
                // two-state storage they are constant zero.
                let signed = expr_context.signed;
                let zero = |zw: usize| -> (Box<ProtoExpression>, usize, usize) {
                    (
                        Box::new(ProtoExpression::Value {
                            value: Value::new(0, zw, false),
                            width: zw,
                            expr_context: crate::ir::expression::ExpressionContext {
                                width: zw,
                                signed: false,
                            },
                        }),
                        1,
                        zw,
                    )
                };
                let mut elements: Vec<(Box<ProtoExpression>, usize, usize)> = Vec::new();
                let mut hi = msb; // highest bit not yet emitted
                let mut done = false;
                for f in sv.overlapping(msb, lsb).iter().rev() {
                    let seg_hi = hi.min(f.1);
                    if seg_hi < hi {
                        elements.push(zero(hi - seg_hi));
                    }
                    let seg_lo = f.0.max(lsb);
                    elements.push((
                        Box::new(field_read(f, seg_hi, seg_lo, false)),
                        1,
                        seg_hi - seg_lo + 1,
                    ));
                    if seg_lo == lsb {
                        done = true;
                        break;
                    }
                    hi = seg_lo - 1;
                }
                if !done {
                    // Trailing zero bits below the lowest covered field.
                    elements.push(zero(hi - lsb + 1));
                }
                let w = *width;
                *e = ProtoExpression::Concatenation {
                    elements,
                    width: w,
                    expr_context: crate::ir::expression::ExpressionContext { width: w, signed },
                };
                self.stats.gathers += 1;
            }
            ProtoExpression::Value { .. } => {}
            ProtoExpression::Unary { x, .. } => self.expr(x),
            ProtoExpression::Binary { x, y, .. } => {
                self.expr(x);
                self.expr(y);
            }
            ProtoExpression::Concatenation { elements, .. } => {
                for (x, _, _) in elements {
                    self.expr(x);
                }
            }
            ProtoExpression::Ternary {
                cond,
                true_expr,
                false_expr,
                ..
            } => {
                self.expr(cond);
                self.expr(true_expr);
                self.expr(false_expr);
            }
            ProtoExpression::DynamicVariable {
                index_expr,
                dynamic_select,
                ..
            } => {
                self.expr(index_expr);
                if let Some(ds) = dynamic_select {
                    self.expr(&mut ds.index_expr);
                }
            }
            ProtoExpression::HierVariable(_) => {}
        }
    }

    fn mapped(&self, off: &VarOffset) -> bool {
        matches!(off, VarOffset::Comb(o) if self.map.contains_key(o))
    }

    fn assign(&mut self, a: &mut ProtoAssignStatement) {
        if let Some(ds) = &mut a.dynamic_select {
            self.expr(&mut ds.index_expr);
        }
        self.expr(&mut a.expr);
        let VarOffset::Comb(o) = a.dst else { return };
        let Some(sv) = self.map.get(&o) else { return };
        debug_assert!(a.dynamic_select.is_none());
        let Some((msb, lsb)) = a.select else {
            debug_assert!(false, "whole write to a split variable");
            return;
        };
        // Candidacy guarantees every write range is contained in a field.
        let Some(f) = sv.containing(msb, lsb) else {
            debug_assert!(false, "write outside every field of a split variable");
            return;
        };
        let (off, fw, sel) = rebase(f, msb, lsb);
        a.dst = VarOffset::Comb(off);
        a.dst_width = fw;
        a.select = sel;
        self.stats.writes_rewritten += 1;
    }

    fn stmt(&mut self, s: &mut ProtoStatement) {
        match s {
            ProtoStatement::Assign(a) => self.assign(a),
            ProtoStatement::AssignDynamic(a) => {
                self.expr(&mut a.dst_index_expr);
                self.expr(&mut a.expr);
                if let Some(ds) = &mut a.dynamic_select {
                    self.expr(&mut ds.index_expr);
                }
            }
            ProtoStatement::If(x) => {
                if let Some(cond) = &mut x.cond {
                    self.expr(cond);
                }
                for s in x.true_side.iter_mut().chain(x.false_side.iter_mut()) {
                    self.stmt(s);
                }
            }
            ProtoStatement::Case(x) => {
                for arm in &mut x.arms {
                    self.expr(&mut arm.cond);
                    for s in &mut arm.body {
                        self.stmt(s);
                    }
                }
                for s in &mut x.default {
                    self.stmt(s);
                }
            }
            ProtoStatement::For(x) => {
                let (start, end) = match &mut x.range {
                    ProtoForRange::Forward { start, end, .. }
                    | ProtoForRange::Reverse { start, end, .. }
                    | ProtoForRange::Stepped { start, end, .. } => (start, end),
                };
                for b in [start, end] {
                    if let ProtoForBound::Dynamic(e) = b {
                        self.expr(e);
                    }
                }
                for s in &mut x.body {
                    self.stmt(s);
                }
            }
            ProtoStatement::SequentialBlock(body) => {
                for s in body {
                    self.stmt(s);
                }
            }
            ProtoStatement::SystemFunctionCall(x) => match x {
                ProtoSystemFunctionCall::Display { args, .. }
                | ProtoSystemFunctionCall::Write { args, .. } => {
                    for a in args {
                        self.expr(a);
                    }
                }
                ProtoSystemFunctionCall::Assert {
                    condition, args, ..
                } => {
                    self.expr(condition);
                    for a in args {
                        self.expr(a);
                    }
                }
                ProtoSystemFunctionCall::Readmemh { .. } | ProtoSystemFunctionCall::Finish => {}
            },
            ProtoStatement::CompiledBlock(_)
            | ProtoStatement::TbMethodCall { .. }
            | ProtoStatement::Break => {}
        }
    }
}

/// Run the pass over the unified comb list.  `event_statements` (the initial
/// block included) is scanned, never rewritten: whatever it touches stays
/// packed.  `blocklist` carries the caller-known externally-visible offsets.
/// New storage comes from `alloc` and is registered in `comb_reloc` so cone
/// gating attributes it to the packed variable's owner.
pub fn run(
    unified: &mut [ProtoStatement],
    event_statements: &HashMap<crate::ir::Event, Vec<ProtoStatement>>,
    blocklist: &HashSet<isize>,
    alloc: &mut dyn FnMut(usize) -> isize,
    comb_reloc: &mut Vec<(isize, isize, usize)>,
    use_4state: bool,
) -> RunStats {
    let mut stats = RunStats::default();
    let mut c = Census::default();
    for s in unified.iter() {
        census_stmt(s, &mut c, false);
    }
    for stmts in event_statements.values() {
        for s in stmts {
            census_stmt(s, &mut c, true);
        }
    }
    if c.bail {
        return stats;
    }
    // Coalesced into a disjoint cover so each candidate answers with one
    // binary search rather than a scan of every dynamic-access site.
    c.dyn_spans.sort_unstable();
    let dyn_cover: Vec<(isize, isize)> = {
        let mut out: Vec<(isize, isize)> = Vec::with_capacity(c.dyn_spans.len());
        for &(a, b) in &c.dyn_spans {
            match out.last_mut() {
                Some(last) if a <= last.1 => last.1 = last.1.max(b),
                _ => out.push((a, b)),
            }
        }
        out
    };
    let in_dyn_span = |start: isize, end: isize| -> bool {
        let i = dyn_cover.partition_point(|&(s, _)| s < end);
        i.checked_sub(1).is_some_and(|i| dyn_cover[i].1 > start)
    };

    let gather_limit = gather_limit();
    let mut offsets: Vec<isize> = c.vars.keys().copied().collect();
    offsets.sort_unstable();
    let mut map: HashMap<isize, SplitVar> = HashMap::default();
    'var: for o in offsets {
        let v = &c.vars[&o];
        if v.disqualified || v.width_conflict || v.full_width == 0 {
            stats.skip_disq += 1;
            continue;
        }
        if blocklist.contains(&o) {
            stats.skip_blocklist += 1;
            continue;
        }
        if v.whole_write || v.write_ranges.is_empty() {
            stats.skip_whole_write += 1;
            continue;
        }
        let footprint = value_size(native_bytes(v.full_width), use_4state) as isize;
        if in_dyn_span(o, o + footprint) {
            stats.skip_disq += 1;
            continue;
        }
        // Merge overlapping (not merely adjacent) write ranges into fields.
        let mut ranges = v.write_ranges.clone();
        ranges.sort_unstable_by_key(|&(msb, lsb)| (lsb, msb));
        let mut fields: Vec<(usize, usize)> = Vec::new(); // (lsb, msb)
        for (msb, lsb) in ranges {
            match fields.last_mut() {
                Some(last) if lsb <= last.1 => last.1 = last.1.max(msb),
                _ => fields.push((lsb, msb)),
            }
        }
        if fields.len() < 2 {
            stats.skip_few_fields += 1;
            continue;
        }
        if fields.iter().any(|&(lo, hi)| hi - lo + 1 > 64) {
            stats.skip_wide_field += 1;
            continue;
        }
        // Every spanning read must gather at most `gather_limit` fields.
        for &(msb, lsb) in &v.read_ranges {
            if fields.iter().any(|&(lo, hi)| lo <= lsb && msb <= hi) {
                continue;
            }
            let n = fields
                .iter()
                .filter(|&&(lo, hi)| lo <= msb && lsb <= hi)
                .count();
            if n > gather_limit {
                stats.skip_gather += 1;
                continue 'var;
            }
        }
        let split = SplitVar {
            fields: fields
                .iter()
                .map(|&(lo, hi)| {
                    let fw = hi - lo + 1;
                    let off = alloc(fw);
                    comb_reloc.push((o, off, value_size(native_bytes(fw), use_4state)));
                    (lo, hi, off, fw)
                })
                .collect(),
        };
        stats.vars_split += 1;
        stats.fields += split.fields.len();
        map.insert(o, split);
    }
    if map.is_empty() {
        if diag() {
            eprintln!("[field_unfuse] no candidates: {stats:?}");
        }
        return stats;
    }

    let mut rw = Rewriter {
        map: &map,
        stats: &mut stats,
    };
    for s in unified.iter_mut() {
        rw.stmt(s);
    }
    if diag() {
        let mut offs: Vec<_> = map.keys().collect();
        offs.sort_unstable();
        eprintln!(
            "[field_unfuse] split={} fields={} writes={} reads={} gathers={} offsets={:?}",
            stats.vars_split,
            stats.fields,
            stats.writes_rewritten,
            stats.reads_rewritten,
            stats.gathers,
            offs
        );
        eprintln!("[field_unfuse] {stats:?}");
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::statement::ProtoTbMethodKind;
    use crate::ir::{ExpressionContext, ProtoExpression};
    use veryl_parser::resource_table::StrId;

    fn comb_read(off: isize, w: usize) -> ProtoExpression {
        ProtoExpression::Variable {
            var_offset: VarOffset::Comb(off),
            select: None,
            dynamic_select: None,
            width: w,
            var_full_width: w,
            expr_context: ExpressionContext {
                width: w,
                signed: false,
            },
        }
    }

    /// A `$tb` method ARGUMENT can carry a hierarchically-resolved DUT comb
    /// read no blocklist covers; the poison scan must disqualify it, or the
    /// variable splits out from under the baked interpreter offset.
    #[test]
    fn tb_method_args_poison_their_reads() {
        let stmt = ProtoStatement::TbMethodCall {
            inst: StrId::default(),
            method: ProtoTbMethodKind::FileWrite {
                format_str: "%x".into(),
                args: vec![comb_read(0x10, 96)],
            },
        };
        let mut c = Census::default();
        census_stmt(&stmt, &mut c, true);
        assert!(
            c.vars.get(&0x10).is_some_and(|v| v.disqualified),
            "a FileWrite arg read must poison its comb variable"
        );
        assert!(!c.bail, "the poison scan must not bail on a tb method");
    }
}
