//! Per-statement comb dependency collection.
//!
//! Records which comb/ff words a statement reads and writes, at the byte
//! granularity the reader's declared width gives.  Two consumers: the
//! version split, to order writers against readers within a block, and
//! [`stmts_infeasible`], to size a block's read expansion.

use crate::ir::ProtoSystemFunctionCall;
use crate::ir::statement::ProtoStatement;
use crate::ir::variable::VarOffset;

/// One read or written variable.  `bytes` is `None` where no span is known
/// (Readmemh elements, deps-less CompiledBlock fallbacks); the consumer
/// then assumes a default.
#[derive(Debug, Clone)]
pub struct Dep {
    pub off: VarOffset,
    pub bytes: Option<u32>,
}

#[derive(Debug, Default)]
pub struct StmtDeps {
    pub ins: Vec<Dep>,
    pub outs: Vec<Dep>,
}

/// Walk one statement, appending its reads/writes.
///
/// Coverage is full, unlike the base+last array encoding `analyze_dependency`
/// uses: a dynamically-indexed access records the whole array range.
pub fn collect_stmt_deps(stmt: &ProtoStatement, deps: &mut StmtDeps) {
    match stmt {
        ProtoStatement::Assign(x) => {
            let mut ins = Vec::new();
            x.expr.gather_reads_expanded_ranged(&mut ins);
            if let Some(ds) = &x.dynamic_select {
                ds.index_expr.gather_reads_expanded_ranged(&mut ins);
            }
            deps.ins.extend(ins.into_iter().map(|(off, _, nb)| Dep {
                off,
                bytes: Some(nb as u32),
            }));
            deps.outs.push(Dep {
                off: x.dst,
                bytes: Some(crate::ir::variable::native_bytes(x.dst_width) as u32),
            });
        }
        ProtoStatement::AssignDynamic(x) => {
            let mut ins = Vec::new();
            x.dst_index_expr.gather_reads_expanded_ranged(&mut ins);
            x.expr.gather_reads_expanded_ranged(&mut ins);
            if let Some(ds) = &x.dynamic_select {
                ds.index_expr.gather_reads_expanded_ranged(&mut ins);
            }
            deps.ins.extend(ins.into_iter().map(|(off, _, nb)| Dep {
                off,
                bytes: Some(nb as u32),
            }));
            if x.dst_stride < 0 || x.dst_num_elements == 0 {
                // Unmodelable span: record no write rather than a partial one.
                return;
            }
            // Any element may be written, and the stride is at least the
            // element's native width, so stride×n covers the whole array.
            let span = x.dst_stride as usize * x.dst_num_elements;
            deps.outs.push(Dep {
                off: x.dst_base,
                bytes: Some(span as u32),
            });
        }
        ProtoStatement::If(x) => {
            if let Some(cond) = &x.cond {
                let mut ins = Vec::new();
                cond.gather_reads_expanded_ranged(&mut ins);
                deps.ins.extend(ins.into_iter().map(|(off, _, nb)| Dep {
                    off,
                    bytes: Some(nb as u32),
                }));
            }
            for s in &x.true_side {
                collect_stmt_deps(s, deps);
            }
            for s in &x.false_side {
                collect_stmt_deps(s, deps);
            }
        }
        ProtoStatement::Case(x) => {
            for arm in &x.arms {
                let mut ins = Vec::new();
                arm.cond.gather_reads_expanded_ranged(&mut ins);
                deps.ins.extend(ins.into_iter().map(|(off, _, nb)| Dep {
                    off,
                    bytes: Some(nb as u32),
                }));
                for s in &arm.body {
                    collect_stmt_deps(s, deps);
                }
            }
            for s in &x.default {
                collect_stmt_deps(s, deps);
            }
        }
        ProtoStatement::For(x) => {
            let mut ins = Vec::new();
            for e in x.range.dynamic_bounds() {
                e.gather_reads_expanded_ranged(&mut ins);
            }
            deps.ins.extend(ins.into_iter().map(|(off, _, nb)| Dep {
                off,
                bytes: Some(nb as u32),
            }));
            deps.outs.push(Dep {
                off: x.var_offset,
                bytes: Some(x.var_native_bytes as u32),
            });
            for s in &x.body {
                collect_stmt_deps(s, deps);
            }
        }
        ProtoStatement::Break => {}
        ProtoStatement::SystemFunctionCall(x) => {
            // A testbench-driven `$readmemh` can run mid-simulation.
            if let ProtoSystemFunctionCall::Readmemh { elements, .. } = x {
                for elem in elements {
                    deps.outs.push(Dep {
                        off: elem.current,
                        bytes: None,
                    });
                }
            }
        }
        ProtoStatement::CompiledBlock(x) => {
            if !x.original_stmts.is_empty() {
                for s in &x.original_stmts {
                    collect_stmt_deps(s, deps);
                }
            } else {
                // Analyzer-grade sets: base+last encoded for arrays, so
                // slightly under-covered.
                deps.ins
                    .extend(x.input_offsets.iter().map(|&off| Dep { off, bytes: None }));
                deps.outs
                    .extend(x.output_offsets.iter().map(|&off| Dep { off, bytes: None }));
            }
        }
        ProtoStatement::SequentialBlock(body) => {
            // No block-internal hiding (unlike `gather_variable_offsets`):
            // a word read before it is written inside the block is a real
            // external read, and keeping written words in the read set only
            // costs a self-edge.
            for s in body {
                collect_stmt_deps(s, deps);
            }
        }
        ProtoStatement::TbMethodCall { .. } => {}
    }
}

/// Above this, a statement is a whole-memory reader.
const MAX_ENTRY_IN_WORDS: usize = 1 << 20;

/// Total expansion budget across a block.
const MAX_BLOCK_IN_WORDS: usize = 1 << 24;

/// Whether a block contains a statement whose read (or write) expansion is
/// too large for the select-chain transforms to be worth running.
pub fn stmts_infeasible(stmts: &[ProtoStatement]) -> bool {
    fn dep_words_approx(d: &Dep) -> usize {
        let o = match d.off {
            VarOffset::Comb(o) | VarOffset::Ff(o) => o,
        };
        if o < 0 {
            return 0;
        }
        let bytes = d.bytes.map(|b| b as usize).unwrap_or(8).max(1);
        (o as usize + bytes).div_ceil(8) - o as usize / 8
    }
    fn walk(stmts: &[ProtoStatement], total: &mut usize) -> bool {
        for s in stmts {
            match s {
                ProtoStatement::CompiledBlock(cb) if !cb.original_stmts.is_empty() => {
                    if walk(&cb.original_stmts, total) {
                        return true;
                    }
                }
                ProtoStatement::SequentialBlock(body) => {
                    if walk(body, total) {
                        return true;
                    }
                }
                s => {
                    let mut deps = StmtDeps::default();
                    collect_stmt_deps(s, &mut deps);
                    let est_out: usize = deps.outs.iter().map(dep_words_approx).sum();
                    if est_out > MAX_ENTRY_IN_WORDS {
                        return true;
                    }
                    let est_in: usize = deps.ins.iter().map(dep_words_approx).sum();
                    if est_in <= MAX_ENTRY_IN_WORDS {
                        *total += est_in;
                        if *total > MAX_BLOCK_IN_WORDS {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }
    let mut total = 0usize;
    walk(stmts, &mut total)
}
