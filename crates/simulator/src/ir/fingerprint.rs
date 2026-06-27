//! Structural fingerprint of an `air::Module`.
//!
//! Two test tops that contain identical hardware logic but differ only in
//! `$readmemh` filename literals, file-open path literals, module names, and
//! `VarId`/`SymbolId` numbering must yield the *same* 32-byte fingerprint, so
//! that the simulator can build a `ProtoModule` once and share it across the
//! matching tests.
//!
//! Design rules (kept deliberately conservative):
//!
//! * `VarId`/`SymbolId` are normalized to a dense appearance-order index via a
//!   single remap each, so renumbering between compilations is absorbed.
//! * Tokens / `TokenRange` and module names (`StrId`) are never folded, since
//!   they vary per file / per test.
//! * String-typed `Comptime` values (the `$readmemh` filename and the file-open
//!   path are both stored as `String` expression literals) are reduced to a
//!   single marker byte, so their content is excluded.
//! * Anything that cannot be normalized or excluded with confidence
//!   (`*::Unsupported`, `Factor::Unknown`, `Component::Interface` /
//!   `Component::SystemVerilog`, excessive recursion depth) returns `None`,
//!   so the caller falls back to building the module individually rather than
//!   risk sharing two structurally different modules.
//!
//! Hash-derived leaf types (`Type`, `ValueVariant`, `Op`, `VarPath`, `Shape`,
//! `Direction`, ...) are folded by running them through an `FxHasher` and
//! feeding the resulting `u64` into the BLAKE3 hasher.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use fxhash::FxHasher;
use veryl_analyzer::ir as air;
use veryl_analyzer::symbol::{ClockDomain, SymbolId};

/// Guards against unbounded recursion through instantiated sub-modules. A
/// finite elaborated design never legitimately exceeds this; hitting it just
/// declines to share (returns `None`), which is safe.
const MAX_DEPTH: usize = 1024;

/// Compute a structural fingerprint of `module`.
///
/// Returns `None` when the module contains a construct that cannot be folded
/// safely (see module docs); the caller should then build the module without
/// proto sharing.
pub fn structural_fingerprint(module: &veryl_analyzer::ir::Module) -> Option<[u8; 32]> {
    let mut fp = Fingerprinter {
        hasher: blake3::Hasher::new(),
        var_remap: HashMap::new(),
        sym_remap: HashMap::new(),
        vars: &module.variables,
        depth: 0,
    };
    // Domain-separation / version tag so the scheme can evolve.
    fp.hasher.update(b"veryl-air-fingerprint-v1");
    fp.fold_module(module)?;
    Some(fp.hasher.finalize().into())
}

struct Fingerprinter<'a> {
    hasher: blake3::Hasher,
    /// `VarId` -> dense appearance-order index (single map, no scoping).
    var_remap: HashMap<air::VarId, u32>,
    /// `SymbolId` -> dense appearance-order index (single map, no scoping).
    sym_remap: HashMap<SymbolId, u32>,
    /// Variables of the module currently being walked; used to fold a
    /// variable's `VarKind`/`Type` on first encounter of its `VarId`.
    vars: &'a fxhash::FxHashMap<air::VarId, air::Variable>,
    depth: usize,
}

impl<'a> Fingerprinter<'a> {
    // ---- low-level fold helpers -------------------------------------------

    fn fold_u8(&mut self, v: u8) {
        self.hasher.update(&[v]);
    }

    fn fold_u64(&mut self, v: u64) {
        self.hasher.update(&v.to_le_bytes());
    }

    fn fold_bool(&mut self, v: bool) {
        self.fold_u8(v as u8);
    }

    fn fold_hash<H: Hash>(&mut self, x: &H) {
        let mut h = FxHasher::default();
        x.hash(&mut h);
        self.fold_u64(h.finish());
    }

    /// Fold a `VarId` as its normalized appearance-order index. On first
    /// encounter, also fold the referenced variable's `VarKind`/`Type` (when
    /// present in the current module's variable table) for extra precision.
    fn fold_var_id(&mut self, id: air::VarId) {
        let next = self.var_remap.len() as u32;
        let idx = *self.var_remap.entry(id).or_insert(next);
        self.fold_u64(idx as u64);
        if idx == next {
            let info = self.vars.get(&id).map(|v| (v.kind, v.r#type.clone()));
            match info {
                Some((kind, ty)) => {
                    self.fold_u8(1);
                    self.fold_var_kind(kind);
                    self.fold_hash(&ty);
                }
                None => self.fold_u8(0),
            }
        }
    }

    /// Fold a `SymbolId` as its normalized appearance-order index.
    fn fold_sym_id(&mut self, id: SymbolId) {
        let next = self.sym_remap.len() as u32;
        let idx = *self.sym_remap.entry(id).or_insert(next);
        self.fold_u64(idx as u64);
    }

    fn fold_var_kind(&mut self, kind: air::VarKind) {
        let tag = match kind {
            air::VarKind::Param => 0u8,
            air::VarKind::Const => 1,
            air::VarKind::Input => 2,
            air::VarKind::Output => 3,
            air::VarKind::Inout => 4,
            air::VarKind::Variable => 5,
            air::VarKind::Let => 6,
        };
        self.fold_u8(tag);
    }

    fn fold_clock_domain(&mut self, cd: ClockDomain) {
        match cd {
            ClockDomain::Explicit(id) => {
                self.fold_u8(0);
                self.fold_sym_id(id);
            }
            ClockDomain::Inferred(id) => {
                self.fold_u8(1);
                self.fold_sym_id(id);
            }
            ClockDomain::Implicit => self.fold_u8(2),
            ClockDomain::None => self.fold_u8(3),
        }
    }

    // ---- comptime ---------------------------------------------------------

    /// Fold a `Comptime`. String-typed comptimes (file names / paths) collapse
    /// to a single marker so their value/type are excluded. Tokens are never
    /// folded. `SymbolId`s inside the clock domain are normalized.
    fn fold_comptime(&mut self, c: &air::Comptime) {
        if matches!(c.r#type.kind, air::TypeKind::String) {
            self.fold_u8(0xFF);
            return;
        }
        self.fold_u8(0x01);
        self.fold_hash(&c.value);
        self.fold_hash(&c.r#type);
        self.fold_bool(c.is_const);
        self.fold_bool(c.is_global);
        // expr_context (no Hash derive; fold fields directly).
        self.fold_u64(c.expr_context.width as u64);
        self.fold_bool(c.expr_context.signed);
        self.fold_bool(c.expr_context.is_const);
        self.fold_bool(c.expr_context.is_global);
        self.fold_clock_domain(c.clock_domain);
        match &c.part_select {
            Some(ps) => {
                self.fold_u8(1);
                self.fold_part_select_path(ps);
            }
            None => self.fold_u8(0),
        }
        self.fold_bool(c.evaluated);
    }

    fn fold_part_select_path(&mut self, ps: &air::PartSelectPath) {
        self.fold_hash(&ps.base);
        self.fold_hash(&ps.path);
        self.fold_u64(ps.part_select.len() as u64);
        for p in &ps.part_select {
            self.fold_u64(p.pos as u64);
            self.fold_hash(&p.r#type);
        }
    }

    // ---- expressions ------------------------------------------------------

    fn fold_expression(&mut self, e: &air::Expression) -> Option<()> {
        match e {
            air::Expression::Term(factor) => {
                self.fold_u8(0);
                self.fold_factor(factor)?;
            }
            air::Expression::Unary(op, x, c) => {
                self.fold_u8(1);
                self.fold_hash(op);
                self.fold_expression(x)?;
                self.fold_comptime(c);
            }
            air::Expression::Binary(x, op, y, c) => {
                self.fold_u8(2);
                self.fold_hash(op);
                self.fold_expression(x)?;
                self.fold_expression(y)?;
                self.fold_comptime(c);
            }
            air::Expression::Ternary(x, y, z, c) => {
                self.fold_u8(3);
                self.fold_expression(x)?;
                self.fold_expression(y)?;
                self.fold_expression(z)?;
                self.fold_comptime(c);
            }
            air::Expression::Concatenation(items, c) => {
                self.fold_u8(4);
                self.fold_u64(items.len() as u64);
                for (x, rep) in items {
                    self.fold_expression(x)?;
                    match rep {
                        Some(r) => {
                            self.fold_u8(1);
                            self.fold_expression(r)?;
                        }
                        None => self.fold_u8(0),
                    }
                }
                self.fold_comptime(c);
            }
            air::Expression::ArrayLiteral(items, c) => {
                self.fold_u8(5);
                self.fold_u64(items.len() as u64);
                for item in items {
                    self.fold_array_literal_item(item)?;
                }
                self.fold_comptime(c);
            }
            air::Expression::StructConstructor(ty, fields, c) => {
                self.fold_u8(6);
                self.fold_hash(ty);
                self.fold_u64(fields.len() as u64);
                for (name, expr) in fields {
                    self.fold_hash(name);
                    self.fold_expression(expr)?;
                }
                self.fold_comptime(c);
            }
        }
        Some(())
    }

    fn fold_array_literal_item(&mut self, item: &air::ArrayLiteralItem) -> Option<()> {
        match item {
            air::ArrayLiteralItem::Value(x, y) => {
                self.fold_u8(0);
                self.fold_expression(x)?;
                match y {
                    Some(e) => {
                        self.fold_u8(1);
                        self.fold_expression(e)?;
                    }
                    None => self.fold_u8(0),
                }
            }
            air::ArrayLiteralItem::Defaul(x) => {
                self.fold_u8(1);
                self.fold_expression(x)?;
            }
        }
        Some(())
    }

    fn fold_factor(&mut self, f: &air::Factor) -> Option<()> {
        match f {
            air::Factor::Variable(id, index, select, c) => {
                self.fold_u8(0);
                self.fold_var_id(*id);
                self.fold_var_index(index)?;
                self.fold_var_select(select)?;
                self.fold_comptime(c);
            }
            air::Factor::Value(c) => {
                self.fold_u8(1);
                self.fold_comptime(c);
            }
            air::Factor::SystemFunctionCall(sfc) => {
                self.fold_u8(2);
                self.fold_system_function_call(sfc)?;
            }
            air::Factor::FunctionCall(fc) => {
                self.fold_u8(3);
                self.fold_function_call(fc)?;
            }
            air::Factor::Anonymous(c) => {
                self.fold_u8(4);
                self.fold_comptime(c);
            }
            air::Factor::Unknown(_) => return None,
        }
        Some(())
    }

    fn fold_var_index(&mut self, vi: &air::VarIndex) -> Option<()> {
        self.fold_u64(vi.0.len() as u64);
        for e in &vi.0 {
            self.fold_expression(e)?;
        }
        Some(())
    }

    fn fold_var_select(&mut self, vs: &air::VarSelect) -> Option<()> {
        self.fold_u64(vs.0.len() as u64);
        for e in &vs.0 {
            self.fold_expression(e)?;
        }
        match &vs.1 {
            Some((op, e)) => {
                self.fold_u8(1);
                self.fold_var_select_op(op);
                self.fold_expression(e)?;
            }
            None => self.fold_u8(0),
        }
        Some(())
    }

    fn fold_var_select_op(&mut self, op: &air::VarSelectOp) {
        let tag = match op {
            air::VarSelectOp::Colon => 0u8,
            air::VarSelectOp::PlusColon => 1,
            air::VarSelectOp::MinusColon => 2,
            air::VarSelectOp::Step => 3,
        };
        self.fold_u8(tag);
    }

    // ---- system / user function calls -------------------------------------

    fn fold_input(&mut self, i: &air::SystemFunctionInput) -> Option<()> {
        self.fold_expression(&i.0)
    }

    fn fold_assign_destination(&mut self, d: &air::AssignDestination) -> Option<()> {
        self.fold_var_id(d.id);
        self.fold_hash(&d.path);
        self.fold_var_index(&d.index)?;
        self.fold_var_select(&d.select)?;
        self.fold_comptime(&d.comptime);
        Some(())
    }

    fn fold_system_function_call(&mut self, sfc: &air::SystemFunctionCall) -> Option<()> {
        self.fold_system_function_kind(&sfc.kind)?;
        self.fold_comptime(&sfc.comptime);
        Some(())
    }

    fn fold_system_function_kind(&mut self, k: &air::SystemFunctionKind) -> Option<()> {
        match k {
            air::SystemFunctionKind::Bits(i) => {
                self.fold_u8(0);
                self.fold_input(i)?;
            }
            air::SystemFunctionKind::Size(i) => {
                self.fold_u8(1);
                self.fold_input(i)?;
            }
            air::SystemFunctionKind::Clog2(i) => {
                self.fold_u8(2);
                self.fold_input(i)?;
            }
            air::SystemFunctionKind::Onehot(i) => {
                self.fold_u8(3);
                self.fold_input(i)?;
            }
            air::SystemFunctionKind::Readmemh(i, o) => {
                self.fold_u8(4);
                // `i` is the filename: a String literal, excluded by fold_comptime.
                self.fold_input(i)?;
                self.fold_u64(o.0.len() as u64);
                for d in &o.0 {
                    self.fold_assign_destination(d)?;
                }
            }
            air::SystemFunctionKind::Display(args) => {
                self.fold_u8(5);
                self.fold_u64(args.len() as u64);
                for a in args {
                    self.fold_input(a)?;
                }
            }
            air::SystemFunctionKind::Write(args) => {
                self.fold_u8(6);
                self.fold_u64(args.len() as u64);
                for a in args {
                    self.fold_input(a)?;
                }
            }
            air::SystemFunctionKind::Assert { kind, cond, args } => {
                self.fold_u8(7);
                self.fold_assert_kind(*kind);
                self.fold_input(cond)?;
                self.fold_u64(args.len() as u64);
                for a in args {
                    self.fold_input(a)?;
                }
            }
            air::SystemFunctionKind::Finish => self.fold_u8(8),
            air::SystemFunctionKind::Signed(i) => {
                self.fold_u8(9);
                self.fold_input(i)?;
            }
            air::SystemFunctionKind::Unsigned(i) => {
                self.fold_u8(10);
                self.fold_input(i)?;
            }
        }
        Some(())
    }

    fn fold_assert_kind(&mut self, kind: air::AssertKind) {
        let tag = match kind {
            air::AssertKind::Fatal => 0u8,
            air::AssertKind::Continue => 1,
        };
        self.fold_u8(tag);
    }

    fn fold_function_call(&mut self, fc: &air::FunctionCall) -> Option<()> {
        self.fold_var_id(fc.id);
        match &fc.index {
            Some(idx) => {
                self.fold_u8(1);
                self.fold_u64(idx.len() as u64);
                for i in idx {
                    self.fold_u64(*i as u64);
                }
            }
            None => self.fold_u8(0),
        }
        self.fold_comptime(&fc.comptime);
        // inputs / outputs are HashMaps: walk in VarPath order for determinism.
        let mut inputs: Vec<_> = fc.inputs.iter().collect();
        inputs.sort_by(|a, b| a.0.cmp(b.0));
        self.fold_u64(inputs.len() as u64);
        for (path, expr) in inputs {
            self.fold_hash(path);
            self.fold_expression(expr)?;
        }
        let mut outputs: Vec<_> = fc.outputs.iter().collect();
        outputs.sort_by(|a, b| a.0.cmp(b.0));
        self.fold_u64(outputs.len() as u64);
        for (path, dsts) in outputs {
            self.fold_hash(path);
            self.fold_u64(dsts.len() as u64);
            for d in dsts {
                self.fold_assign_destination(d)?;
            }
        }
        Some(())
    }

    // ---- statements -------------------------------------------------------

    fn fold_statements(&mut self, stmts: &[air::Statement]) -> Option<()> {
        self.fold_u64(stmts.len() as u64);
        for s in stmts {
            self.fold_statement(s)?;
        }
        Some(())
    }

    fn fold_statement(&mut self, s: &air::Statement) -> Option<()> {
        match s {
            air::Statement::Assign(a) => {
                self.fold_u8(0);
                self.fold_u64(a.dst.len() as u64);
                for d in &a.dst {
                    self.fold_assign_destination(d)?;
                }
                match a.width {
                    Some(w) => {
                        self.fold_u8(1);
                        self.fold_u64(w as u64);
                    }
                    None => self.fold_u8(0),
                }
                self.fold_expression(&a.expr)?;
            }
            air::Statement::If(i) => {
                self.fold_u8(1);
                self.fold_expression(&i.cond)?;
                self.fold_statements(&i.true_side)?;
                self.fold_statements(&i.false_side)?;
            }
            air::Statement::IfReset(i) => {
                self.fold_u8(2);
                self.fold_statements(&i.true_side)?;
                self.fold_statements(&i.false_side)?;
            }
            air::Statement::Case(c) => {
                self.fold_u8(3);
                self.fold_expression(&c.case_target)?;
                self.fold_u64(c.arms.len() as u64);
                for arm in &c.arms {
                    self.fold_u64(arm.patterns.len() as u64);
                    for p in &arm.patterns {
                        self.fold_case_pattern(p)?;
                    }
                    self.fold_statements(&arm.body)?;
                }
                self.fold_statements(&c.default)?;
            }
            air::Statement::For(fr) => {
                self.fold_u8(4);
                self.fold_var_id(fr.var_id);
                // var_name (StrId) is excluded as a name.
                self.fold_hash(&fr.var_type);
                self.fold_for_range(&fr.range)?;
                self.fold_statements(&fr.body)?;
            }
            air::Statement::SystemFunctionCall(sfc) => {
                self.fold_u8(5);
                self.fold_system_function_call(sfc)?;
            }
            air::Statement::FunctionCall(fc) => {
                self.fold_u8(6);
                self.fold_function_call(fc)?;
            }
            air::Statement::TbMethodCall(tb) => {
                self.fold_u8(7);
                self.fold_hash(&tb.inst);
                self.fold_tb_method(&tb.method)?;
            }
            air::Statement::Break => self.fold_u8(8),
            air::Statement::Unsupported(_) => return None,
            air::Statement::Null => self.fold_u8(9),
        }
        Some(())
    }

    fn fold_case_pattern(&mut self, p: &air::CasePattern) -> Option<()> {
        match p {
            air::CasePattern::Eq(e) => {
                self.fold_u8(0);
                self.fold_expression(e)?;
            }
            air::CasePattern::Range { lo, hi, inclusive } => {
                self.fold_u8(1);
                self.fold_expression(lo)?;
                self.fold_expression(hi)?;
                self.fold_bool(*inclusive);
            }
        }
        Some(())
    }

    fn fold_for_range(&mut self, r: &air::ForRange) -> Option<()> {
        match r {
            air::ForRange::Forward {
                start,
                end,
                inclusive,
                step,
            } => {
                self.fold_u8(0);
                self.fold_for_bound(start)?;
                self.fold_for_bound(end)?;
                self.fold_bool(*inclusive);
                self.fold_u64(*step as u64);
            }
            air::ForRange::Reverse {
                start,
                end,
                inclusive,
                step,
            } => {
                self.fold_u8(1);
                self.fold_for_bound(start)?;
                self.fold_for_bound(end)?;
                self.fold_bool(*inclusive);
                self.fold_u64(*step as u64);
            }
            air::ForRange::Stepped {
                start,
                end,
                inclusive,
                step,
                op,
            } => {
                self.fold_u8(2);
                self.fold_for_bound(start)?;
                self.fold_for_bound(end)?;
                self.fold_bool(*inclusive);
                self.fold_u64(*step as u64);
                self.fold_hash(op);
            }
        }
        Some(())
    }

    fn fold_for_bound(&mut self, b: &air::ForBound) -> Option<()> {
        match b {
            air::ForBound::Const(x) => {
                self.fold_u8(0);
                self.fold_u64(*x as u64);
            }
            air::ForBound::Expression(e) => {
                self.fold_u8(1);
                self.fold_expression(e)?;
            }
        }
        Some(())
    }

    fn fold_tb_method(&mut self, m: &air::TbMethod) -> Option<()> {
        match m {
            air::TbMethod::ClockNext { count, period } => {
                self.fold_u8(0);
                match count {
                    Some(e) => {
                        self.fold_u8(1);
                        self.fold_expression(e)?;
                    }
                    None => self.fold_u8(0),
                }
                match period {
                    Some(e) => {
                        self.fold_u8(1);
                        self.fold_expression(e)?;
                    }
                    None => self.fold_u8(0),
                }
            }
            air::TbMethod::ResetAssert { clock, duration } => {
                self.fold_u8(1);
                self.fold_hash(clock);
                match duration {
                    Some(e) => {
                        self.fold_u8(1);
                        self.fold_expression(e)?;
                    }
                    None => self.fold_u8(0),
                }
            }
            air::TbMethod::FileOpen { name, append } => {
                self.fold_u8(2);
                // `name` is the path: a String literal, excluded by fold_comptime.
                self.fold_input(name)?;
                self.fold_bool(*append);
            }
            air::TbMethod::FileWrite { args } => {
                self.fold_u8(3);
                self.fold_u64(args.len() as u64);
                for a in args {
                    self.fold_input(a)?;
                }
            }
            air::TbMethod::FileClose => self.fold_u8(4),
            air::TbMethod::FileFlush => self.fold_u8(5),
        }
        Some(())
    }

    // ---- declarations -----------------------------------------------------

    fn fold_declaration(&mut self, d: &'a air::Declaration) -> Option<()> {
        match d {
            air::Declaration::Comb(c) => {
                self.fold_u8(0);
                self.fold_statements(&c.statements)?;
            }
            air::Declaration::Ff(ff) => {
                self.fold_u8(1);
                self.fold_ff_clock(&ff.clock)?;
                match &ff.reset {
                    Some(r) => {
                        self.fold_u8(1);
                        self.fold_ff_reset(r)?;
                    }
                    None => self.fold_u8(0),
                }
                self.fold_statements(&ff.statements)?;
            }
            air::Declaration::Inst(inst) => {
                self.fold_u8(2);
                self.fold_inst(inst)?;
            }
            air::Declaration::Initial(i) => {
                self.fold_u8(3);
                self.fold_statements(&i.statements)?;
            }
            air::Declaration::Final(f) => {
                self.fold_u8(4);
                self.fold_statements(&f.statements)?;
            }
            air::Declaration::Unsupported(_) => return None,
            air::Declaration::Null => self.fold_u8(5),
        }
        Some(())
    }

    fn fold_ff_clock(&mut self, c: &air::FfClock) -> Option<()> {
        self.fold_var_id(c.id);
        self.fold_var_index(&c.index)?;
        self.fold_var_select(&c.select)?;
        self.fold_comptime(&c.comptime);
        Some(())
    }

    fn fold_ff_reset(&mut self, r: &air::FfReset) -> Option<()> {
        self.fold_var_id(r.id);
        self.fold_var_index(&r.index)?;
        self.fold_var_select(&r.select)?;
        self.fold_comptime(&r.comptime);
        Some(())
    }

    fn fold_inst(&mut self, inst: &'a air::InstDeclaration) -> Option<()> {
        // `name` and `token` are excluded.
        self.fold_u64(inst.inputs.len() as u64);
        for i in &inst.inputs {
            self.fold_var_id(i.id);
            self.fold_expression(&i.expr)?;
        }
        self.fold_u64(inst.outputs.len() as u64);
        for o in &inst.outputs {
            self.fold_var_id(o.id);
            self.fold_u64(o.dst.len() as u64);
            for d in &o.dst {
                self.fold_assign_destination(d)?;
            }
        }
        self.fold_component(inst.component.as_ref())?;
        Some(())
    }

    fn fold_component(&mut self, c: &'a air::Component) -> Option<()> {
        match c {
            air::Component::Module(m) => {
                self.fold_u8(0);
                self.fold_module(m)?;
            }
            // Identified only by their (excluded) names; folding could merge distinct logic.
            air::Component::Interface(_) => return None,
            air::Component::SystemVerilog(_) => return None,
        }
        Some(())
    }

    // ---- functions --------------------------------------------------------

    fn fold_function(&mut self, func: &air::Function) -> Option<()> {
        // `name`, `path` (whose Signature carries per-test SymbolIds), and
        // `token` are excluded; the call id was already folded by the caller.
        self.fold_comptime(&func.r#type);
        self.fold_hash(&func.array);
        self.fold_u64(func.arity as u64);
        self.fold_u64(func.args.len() as u64);
        for arg in &func.args {
            self.fold_hash(&arg.name);
            self.fold_comptime(&arg.comptime);
            self.fold_u64(arg.members.len() as u64);
            for (path, c, dir) in &arg.members {
                self.fold_hash(path);
                self.fold_comptime(c);
                self.fold_hash(dir);
            }
        }
        self.fold_bool(func.is_const);
        self.fold_u64(func.functions.len() as u64);
        for body in &func.functions {
            match &body.ret {
                Some(id) => {
                    self.fold_u8(1);
                    self.fold_var_id(*id);
                }
                None => self.fold_u8(0),
            }
            let mut arg_map: Vec<_> = body.arg_map.iter().collect();
            arg_map.sort_by(|a, b| a.0.cmp(b.0));
            self.fold_u64(arg_map.len() as u64);
            for (path, id) in arg_map {
                self.fold_hash(path);
                self.fold_var_id(*id);
            }
            self.fold_statements(&body.statements)?;
        }
        Some(())
    }

    // ---- module -----------------------------------------------------------

    fn fold_module(&mut self, m: &'a air::Module) -> Option<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return None;
        }
        // `name` and `token` are excluded. Switch the variable table to this
        // module so per-VarId variable info is folded against the right scope.
        let prev_vars = self.vars;
        self.vars = &m.variables;

        // Ports: walk in VarPath order (HashMap) for determinism.
        let mut ports: Vec<_> = m.ports.iter().collect();
        ports.sort_by(|a, b| a.0.cmp(b.0));
        self.fold_u64(ports.len() as u64);
        for (path, id) in ports {
            self.fold_hash(path);
            self.fold_var_id(*id);
            match m.port_types.get(path) {
                Some((ty, cd)) => {
                    self.fold_u8(1);
                    self.fold_hash(ty);
                    self.fold_clock_domain(*cd);
                }
                None => self.fold_u8(0),
            }
        }

        // Declarations: ordered Vec, walk as-is.
        self.fold_u64(m.declarations.len() as u64);
        for d in &m.declarations {
            self.fold_declaration(d)?;
        }

        // Functions: HashMap keyed by VarId; sort by raw VarId so the relative
        // (definition) order is stable across renumbered compilations.
        let mut funcs: Vec<_> = m.functions.iter().collect();
        funcs.sort_by(|a, b| a.0.cmp(b.0));
        self.fold_u64(funcs.len() as u64);
        for (id, func) in funcs {
            self.fold_var_id(*id);
            self.fold_function(func)?;
        }

        self.vars = prev_vars;
        self.depth -= 1;
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use veryl_analyzer::ir as air;
    use veryl_parser::resource_table;

    fn empty_module(name: &str) -> air::Module {
        air::Module {
            name: resource_table::insert_str(name),
            token: Default::default(),
            ports: Default::default(),
            port_types: Default::default(),
            variables: Default::default(),
            functions: Default::default(),
            declarations: Vec::new(),
            suppress_unassigned: false,
            per_decl_refs: Default::default(),
            assign_tokens: Default::default(),
            ff_table: Default::default(),
        }
    }

    #[test]
    fn fingerprint_ignores_module_name() {
        // Two empty modules that differ only in name must fingerprint equally.
        let a = empty_module("test_0000");
        let b = empty_module("test_0005");
        let fa = structural_fingerprint(&a).expect("fingerprint a");
        let fb = structural_fingerprint(&b).expect("fingerprint b");
        assert_eq!(fa, fb);

        // And the fingerprint is stable across repeated calls.
        assert_eq!(fa, structural_fingerprint(&a).unwrap());
    }

    #[test]
    fn fingerprint_distinguishes_structure() {
        // A module with a (Null) declaration must differ from an empty one.
        let mut a = empty_module("m");
        a.declarations.push(air::Declaration::Null);
        let b = empty_module("m");
        assert_ne!(
            structural_fingerprint(&a).unwrap(),
            structural_fingerprint(&b).unwrap()
        );
    }

    #[test]
    fn unsupported_declaration_yields_none() {
        let mut a = empty_module("m");
        a.declarations
            .push(air::Declaration::Unsupported(Default::default()));
        assert!(structural_fingerprint(&a).is_none());
    }
}
