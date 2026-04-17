use crate::HashMap;
use crate::ir::{
    Event, Expression, Ir, ModuleVariables, RuntimeForRange, Statement, SystemFunctionCall,
    TbMethodKind, Value, VarId, VarPath, write_native_value,
};
use crate::simulator::Simulator;
use crate::simulator_error::SimulatorError;
use crate::wave_dumper::WaveDumper;
use veryl_analyzer::ir::ControlFlow;
use veryl_analyzer::value::MaskCache;
use veryl_parser::resource_table::StrId;

pub enum TestbenchStatement {
    /// Normal simulator statement (assign, $display, etc.)
    Stmt(Statement),
    /// clk.next(N)
    ClockNext {
        clock: Event,
        count: Option<Expression>,
        high_time: u64,
        low_time: u64,
    },
    /// rst.assert(clk, N)
    ResetAssert {
        reset: Event,
        clock: Event,
        duration: u64,
        high_time: u64,
        low_time: u64,
    },
    /// $assert(cond, msg)
    Assert {
        condition: Expression,
        message: Option<String>,
    },
    /// if-else (may contain next inside)
    If {
        condition: Expression,
        then_block: Vec<TestbenchStatement>,
        else_block: Vec<TestbenchStatement>,
    },
    /// for loop with fixed count
    For {
        count: u64,
        body: Vec<TestbenchStatement>,
        loop_var: Option<LoopVariable>,
    },
    /// $finish
    Finish,
}

pub struct LoopVariable {
    pub ptr: *mut u8,
    pub native_bytes: usize,
    pub use_4state: bool,
    pub width: usize,
    pub signed: bool,
    pub range: RuntimeForRange,
}

#[derive(Debug, PartialEq, Eq)]
pub enum TestResult {
    Pass,
    Fail(String),
}

/// Internal execution result that distinguishes Finish from normal continuation.
#[derive(Debug, PartialEq, Eq)]
enum ExecResult {
    Continue,
    Break,
    Finished,
    Fail(String),
}

impl ExecResult {
    fn should_stop(&self) -> bool {
        !matches!(self, ExecResult::Continue)
    }
}

impl From<ExecResult> for TestResult {
    fn from(result: ExecResult) -> Self {
        match result {
            ExecResult::Continue | ExecResult::Break | ExecResult::Finished => TestResult::Pass,
            ExecResult::Fail(msg) => TestResult::Fail(msg),
        }
    }
}

fn find_var_id_by_name(module_variables: &ModuleVariables, name: StrId) -> Option<VarId> {
    let target_path = VarPath::new(name);
    for (var_id, variable) in &module_variables.variables {
        if variable.path == target_path {
            return Some(*var_id);
        }
    }
    for child in &module_variables.children {
        if let Some(id) = find_var_id_by_name(child, name) {
            return Some(id);
        }
    }
    None
}

/// Recursively collect $tb method call instances from statements,
/// including those nested inside For loops and If blocks.
fn collect_tb_insts(
    stmts: &[Statement],
    clock_insts: &mut Vec<StrId>,
    reset_insts: &mut Vec<StrId>,
) {
    for stmt in stmts {
        match stmt {
            Statement::TbMethodCall { inst, method } => match method {
                TbMethodKind::ClockNext { .. } => {
                    if !clock_insts.contains(inst) {
                        clock_insts.push(*inst);
                    }
                }
                TbMethodKind::ResetAssert { .. } => {
                    if !reset_insts.contains(inst) {
                        reset_insts.push(*inst);
                    }
                }
            },
            Statement::For(for_stmt) => {
                collect_tb_insts(&for_stmt.body, clock_insts, reset_insts);
            }
            Statement::If(if_stmt) => {
                collect_tb_insts(&if_stmt.true_side, clock_insts, reset_insts);
                collect_tb_insts(&if_stmt.false_side, clock_insts, reset_insts);
            }
            _ => {}
        }
    }
}

/// Build a mapping from $tb instance names (StrId) to their corresponding Events.
pub fn build_event_map(
    event_statements: &HashMap<Event, Vec<Statement>>,
    module_variables: &ModuleVariables,
) -> HashMap<StrId, Event> {
    let mut clock_insts: Vec<StrId> = Vec::new();
    let mut reset_insts: Vec<StrId> = Vec::new();
    for stmts in event_statements.values() {
        collect_tb_insts(stmts, &mut clock_insts, &mut reset_insts);
    }

    let mut event_map = HashMap::default();

    for inst in clock_insts {
        let var_id = find_var_id_by_name(module_variables, inst).unwrap_or(VarId::SYNTHETIC);
        event_map.entry(inst).or_insert(Event::Clock(var_id));
    }

    for inst in reset_insts {
        let var_id = find_var_id_by_name(module_variables, inst).unwrap_or(VarId::SYNTHETIC);
        event_map.entry(inst).or_insert(Event::Reset(var_id));
    }

    event_map
}

/// period < 2 is clamped to 2. Remainder goes to high (posedge) phase.
fn compute_half_periods(period: u64) -> (u64, u64) {
    let p = period.max(2);
    (p.div_ceil(2), p / 2)
}

fn collect_clock_periods(stmts: &[Statement], periods: &mut HashMap<StrId, u64>) {
    for stmt in stmts {
        match stmt {
            Statement::TbMethodCall { inst, method } => {
                if let TbMethodKind::ClockNext { period, .. } = method
                    && let Some(expr) = period
                {
                    let val = expr.eval(&mut MaskCache::default());
                    periods.entry(*inst).or_insert(val.payload_u64());
                }
            }
            Statement::For(for_stmt) => {
                collect_clock_periods(&for_stmt.body, periods);
            }
            Statement::If(if_stmt) => {
                collect_clock_periods(&if_stmt.true_side, periods);
                collect_clock_periods(&if_stmt.false_side, periods);
            }
            _ => {}
        }
    }
}

pub fn build_clock_periods(
    event_statements: &HashMap<Event, Vec<Statement>>,
) -> HashMap<StrId, u64> {
    let mut periods = HashMap::default();
    for stmts in event_statements.values() {
        collect_clock_periods(stmts, &mut periods);
    }
    periods
}

/// Convert a list of simulator Statements (from initial block) into TestbenchStatements.
///
/// `event_map` maps $tb instance names (StrId) to their corresponding Events.
/// For clock_gen instances, the value is Event::Clock(VarId).
/// For reset_gen instances, the value is Event::Reset(VarId).
pub fn convert_initial_to_testbench(
    stmts: &[Statement],
    event_map: &HashMap<StrId, Event>,
    clock_periods: &HashMap<StrId, u64>,
    default_reset_duration: u64,
) -> Vec<TestbenchStatement> {
    let mut result = Vec::new();
    for stmt in stmts {
        result.push(convert_stmt(
            stmt,
            event_map,
            clock_periods,
            default_reset_duration,
        ));
    }
    result
}

fn convert_stmt(
    stmt: &Statement,
    event_map: &HashMap<StrId, Event>,
    clock_periods: &HashMap<StrId, u64>,
    default_reset_duration: u64,
) -> TestbenchStatement {
    match stmt {
        Statement::TbMethodCall { inst, method } => match method {
            TbMethodKind::ClockNext { count, period } => {
                let clock = event_map.get(inst).cloned().unwrap_or(Event::Initial);
                let p = if let Some(expr) = period {
                    let val = expr.eval(&mut MaskCache::default());
                    val.payload_u64()
                } else {
                    2
                };
                let (high_time, low_time) = compute_half_periods(p);
                TestbenchStatement::ClockNext {
                    clock,
                    count: count.clone(),
                    high_time,
                    low_time,
                }
            }
            TbMethodKind::ResetAssert { clock, duration } => {
                let reset = event_map.get(inst).cloned().unwrap_or(Event::Initial);
                let clock_event = event_map.get(clock).cloned().unwrap_or(Event::Initial);
                let dur = if let Some(expr) = duration {
                    let val = expr.eval(&mut MaskCache::default());
                    val.payload_u64().max(1)
                } else {
                    default_reset_duration
                };
                let clock_period = clock_periods.get(clock).copied().unwrap_or(2);
                let (high_time, low_time) = compute_half_periods(clock_period);
                TestbenchStatement::ResetAssert {
                    reset,
                    clock: clock_event,
                    duration: dur,
                    high_time,
                    low_time,
                }
            }
        },
        Statement::SystemFunctionCall(SystemFunctionCall::Assert { condition, message }) => {
            TestbenchStatement::Assert {
                condition: condition.clone(),
                message: message.clone(),
            }
        }
        Statement::SystemFunctionCall(SystemFunctionCall::Finish) => TestbenchStatement::Finish,
        Statement::If(if_stmt) => {
            let then_block = convert_stmts(
                &if_stmt.true_side,
                event_map,
                clock_periods,
                default_reset_duration,
            );
            let else_block = convert_stmts(
                &if_stmt.false_side,
                event_map,
                clock_periods,
                default_reset_duration,
            );
            if let Some(cond) = &if_stmt.cond {
                TestbenchStatement::If {
                    condition: cond.clone(),
                    then_block,
                    else_block,
                }
            } else {
                // if_reset without condition - treat then_block as the reset path
                TestbenchStatement::Stmt(stmt.clone())
            }
        }
        Statement::For(for_stmt) => {
            let body = for_stmt
                .body
                .iter()
                .map(|s| convert_stmt(s, event_map, clock_periods, default_reset_duration))
                .collect();
            TestbenchStatement::For {
                count: 0, // unused when loop_var is Some
                body,
                loop_var: Some(LoopVariable {
                    ptr: for_stmt.var_ptr,
                    native_bytes: for_stmt.var_native_bytes,
                    use_4state: for_stmt.var_use_4state,
                    width: for_stmt.var_width,
                    signed: for_stmt.var_signed,
                    range: for_stmt.range.clone(),
                }),
            }
        }
        other => TestbenchStatement::Stmt(other.clone()),
    }
}

fn convert_stmts(
    stmts: &[Statement],
    event_map: &HashMap<StrId, Event>,
    clock_periods: &HashMap<StrId, u64>,
    default_reset_duration: u64,
) -> Vec<TestbenchStatement> {
    stmts
        .iter()
        .map(|s| convert_stmt(s, event_map, clock_periods, default_reset_duration))
        .collect()
}

pub fn run_testbench(sim: &mut Simulator, stmts: &[TestbenchStatement]) -> TestResult {
    exec(sim, stmts).into()
}

/// Run a native testbench from a simulator IR.
///
/// `module_name` must be pre-resolved from `ir.name` on the main thread
/// because resource_table is thread-local.
pub fn run_native_testbench(
    ir: Ir,
    dump: Option<WaveDumper>,
    module_name: String,
) -> Result<TestResult, SimulatorError> {
    let mut sim = Simulator::new(ir, dump);
    let event_map = build_event_map(&sim.ir.event_statements, &sim.ir.module_variables);
    let clock_periods = build_clock_periods(&sim.ir.event_statements);

    let token = sim.ir.token;
    let initial_stmts = sim
        .ir
        .event_statements
        .get(&Event::Initial)
        .ok_or_else(|| SimulatorError::no_initial_block(&module_name, &token))?;

    let tb_stmts = convert_initial_to_testbench(initial_stmts, &event_map, &clock_periods, 3);
    let sim_start = std::time::Instant::now();
    let result = run_testbench(&mut sim, &tb_stmts);
    let sim_elapsed = sim_start.elapsed();
    log::info!("simulation time: {:.2}s", sim_elapsed.as_secs_f64());

    // Log interpreter statement breakdown
    {
        let mut interp_counts: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        let mut jit_count = 0usize;
        let count_stmts =
            |stmts: &[crate::ir::Statement],
             jit: &mut usize,
             interp: &mut std::collections::HashMap<&str, usize>| {
                for s in stmts {
                    if s.is_binary() {
                        *jit += 1;
                    } else {
                        *interp.entry(s.type_name()).or_default() += 1;
                    }
                }
            };
        let mut comb_jit = 0usize;
        let mut comb_interp: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        count_stmts(&sim.ir.comb_statements, &mut comb_jit, &mut comb_interp);
        jit_count += comb_jit;
        for (k, v) in &comb_interp {
            *interp_counts.entry(k).or_default() += v;
        }
        for (event, stmts) in &sim.ir.event_statements {
            let mut ej = 0usize;
            let mut ei: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
            count_stmts(stmts, &mut ej, &mut ei);
            jit_count += ej;
            for (k, v) in &ei {
                *interp_counts.entry(k).or_default() += v;
            }
            if !ei.is_empty() || ej > 0 {
                let parts: Vec<_> = ei.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
                log::info!(
                    "  event {:?}: jit={}, interp=[{}]",
                    event,
                    ej,
                    parts.join(", ")
                );
            }
        }
        if !interp_counts.is_empty() {
            let mut parts: Vec<_> = interp_counts.iter().collect();
            parts.sort_by_key(|(_, c)| std::cmp::Reverse(**c));
            let interp_str: Vec<_> = parts.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
            log::info!(
                "stmt breakdown: jit={}, interp=[{}]",
                jit_count,
                interp_str.join(", ")
            );
        }
    }

    #[cfg(feature = "profile")]
    {
        let p = &sim.profile;
        eprintln!("=== SimProfile for {} ===", module_name);
        eprintln!("  step_count:          {}", p.step_count);
        eprintln!("  settle_comb_count:   {}", p.settle_comb_count);
        eprintln!("  comb_eval_count:     {}", p.comb_eval_count);
        eprintln!("  extra_pass_count:    {}", p.extra_pass_count);
        eprintln!("  converged_first_try: {}", p.converged_first_try);
        eprintln!(
            "  settle_comb_ns:      {} ({:.2}ms)",
            p.settle_comb_ns,
            p.settle_comb_ns as f64 / 1_000_000.0
        );
        eprintln!(
            "  event_eval_ns:       {} ({:.2}ms)",
            p.event_eval_ns,
            p.event_eval_ns as f64 / 1_000_000.0
        );
        eprintln!(
            "  ff_swap_ns:          {} ({:.2}ms)",
            p.ff_swap_ns,
            p.ff_swap_ns as f64 / 1_000_000.0
        );
        eprintln!(
            "  eval_comb_full_ns:   {} ({:.2}ms)",
            p.eval_comb_full_ns,
            p.eval_comb_full_ns as f64 / 1_000_000.0
        );
        let (jit, total) = sim.ir.jit_stats();
        eprintln!(
            "  jit_stats:           {}/{} ({:.1}%)",
            jit,
            total,
            if total > 0 {
                jit as f64 / total as f64 * 100.0
            } else {
                0.0
            }
        );
        let (fc_total, fc_jit, fc_interp) = sim.ir.comb_stmt_count();
        eprintln!(
            "  comb_stmts:          {} (jit:{}, interp:{})",
            fc_total, fc_jit, fc_interp
        );
        eprintln!("  comb_values_len:     {}", sim.ir.comb_values.len());
        eprintln!("  ff_values_len:       {}", sim.ir.ff_values.len());
        eprintln!("  ff_commit_entries:   {}", sim.ir.ff_commit_entries.len());
        eprintln!("  required_comb_passes:{}", sim.ir.required_comb_passes);
        eprintln!(
            "  event_comb_offsets:  {}",
            sim.ir.event_comb_write_offsets.len()
        );
        eprintln!(
            "  event_comb_dirty:    {} / {} ({:.1}%)",
            p.event_comb_dirty_cycles,
            p.step_count,
            if p.step_count > 0 {
                p.event_comb_dirty_cycles as f64 / p.step_count as f64 * 100.0
            } else {
                0.0
            }
        );
        eprintln!(
            "  event_comb_changed:  {} / {} ({:.1}%)",
            p.event_comb_value_changed_cycles,
            p.event_comb_dirty_cycles,
            if p.event_comb_dirty_cycles > 0 {
                p.event_comb_value_changed_cycles as f64 / p.event_comb_dirty_cycles as f64 * 100.0
            } else {
                0.0
            }
        );
        if !p.event_stmt_ns.is_empty() {
            eprintln!("  event_stmt_ns:");
            // Show event statement types alongside timing
            let clock_event = sim
                .ir
                .event_statements
                .iter()
                .find(|(e, _)| matches!(e, crate::ir::Event::Clock(_)))
                .map(|(_, stmts)| stmts);
            for (i, &ns) in p.event_stmt_ns.iter().enumerate() {
                let type_info = clock_event
                    .and_then(|stmts| stmts.get(i))
                    .map(|s| match s {
                        crate::ir::Statement::BinaryBatch(_, args) => {
                            format!("BinaryBatch({}inst)", args.len())
                        }
                        _ => s.type_name().to_string(),
                    })
                    .unwrap_or_default();
                eprintln!(
                    "    [{}]: {:.2}ms  {}",
                    i,
                    ns as f64 / 1_000_000.0,
                    type_info
                );
            }
        }
        eprintln!("===========================");
    }

    Ok(result)
}

fn exec(sim: &mut Simulator, stmts: &[TestbenchStatement]) -> ExecResult {
    for stmt in stmts {
        let result = exec_one(sim, stmt);
        if result.should_stop() {
            return result;
        }
    }
    ExecResult::Continue
}

fn exec_one(sim: &mut Simulator, stmt: &TestbenchStatement) -> ExecResult {
    match stmt {
        TestbenchStatement::Stmt(s) => {
            sim.ensure_comb_updated();
            let flow = s.eval_step(&mut sim.mask_cache);
            sim.mark_comb_dirty();
            if flow == ControlFlow::Break {
                ExecResult::Break
            } else {
                ExecResult::Continue
            }
        }
        TestbenchStatement::ClockNext {
            clock,
            count,
            high_time,
            low_time,
        } => {
            let n = if let Some(expr) = count {
                sim.ensure_comb_updated();
                let val = expr.eval(&mut sim.mask_cache);
                val.payload_u64().max(1)
            } else {
                1
            };
            let has_dump = sim.dump.is_some();
            if has_dump {
                for _ in 0..n {
                    if let Some(id) = clock.var_id() {
                        sim.set_var_by_id(&id, Value::new(1, 1, false));
                    }
                    sim.step(clock);
                    sim.time += high_time;
                    if let Some(id) = clock.var_id() {
                        sim.set_var_by_id(&id, Value::new(0, 1, false));
                    }
                    sim.dump_variables();
                    sim.time += low_time;
                }
            } else {
                for _ in 0..n {
                    sim.step(clock);
                    sim.time += high_time + low_time;
                }
            }
            ExecResult::Continue
        }
        TestbenchStatement::ResetAssert {
            reset,
            clock,
            duration,
            high_time,
            low_time,
        } => {
            // Step reset event for `duration` cycles.
            // In this simulator, Event::Reset represents a clock edge
            // with reset asserted (executes the if_reset branch of always_ff).
            let has_dump = sim.dump.is_some();
            if has_dump && let Some(id) = reset.var_id() {
                sim.set_var_by_id(&id, Value::new(1, 1, false));
            }
            for _ in 0..*duration {
                if has_dump && let Some(id) = clock.var_id() {
                    sim.set_var_by_id(&id, Value::new(1, 1, false));
                }
                sim.step(reset);
                sim.time += high_time;
                if has_dump {
                    if let Some(id) = clock.var_id() {
                        sim.set_var_by_id(&id, Value::new(0, 1, false));
                    }
                    sim.dump_variables();
                }
                sim.time += low_time;
            }
            if has_dump && let Some(id) = reset.var_id() {
                sim.set_var_by_id(&id, Value::new(0, 1, false));
            }
            ExecResult::Continue
        }
        TestbenchStatement::Assert { condition, message } => {
            sim.ensure_comb_updated();
            let val = condition.eval(&mut sim.mask_cache);
            if val.payload_u64() == 0 {
                let msg = message.as_deref().unwrap_or("assertion failed").to_string();
                ExecResult::Fail(msg)
            } else {
                ExecResult::Continue
            }
        }
        TestbenchStatement::If {
            condition,
            then_block,
            else_block,
        } => {
            sim.ensure_comb_updated();
            let val = condition.eval(&mut sim.mask_cache);
            if val.payload_u64() != 0 {
                exec(sim, then_block)
            } else {
                exec(sim, else_block)
            }
        }
        TestbenchStatement::For {
            count,
            body,
            loop_var,
        } => {
            if let Some(lv) = loop_var {
                let r = &lv.range;
                let start = r.start.eval(&mut sim.mask_cache);
                let mut end = r.end.eval(&mut sim.mask_cache);
                if r.inclusive {
                    end += 1;
                }
                let step = r.step;
                let op = r.op;
                let reverse = r.reverse;
                let mut step_body = |i: u64| -> ExecResult {
                    let val = Value::new(i, lv.width, lv.signed);
                    unsafe {
                        write_native_value(lv.ptr, lv.native_bytes, lv.use_4state, &val);
                    }
                    exec(sim, body)
                };
                let mut loop_result = ExecResult::Continue;
                if reverse {
                    let mut i = end;
                    while i > start {
                        i -= step;
                        let result = step_body(i);
                        if result.should_stop() {
                            loop_result = result;
                            break;
                        }
                    }
                } else if let Some(op) = op {
                    let mut i = start;
                    while i < end {
                        let result = step_body(i);
                        if result.should_stop() {
                            loop_result = result;
                            break;
                        }
                        i = op.eval(i as usize, step as usize) as u64;
                    }
                } else {
                    let mut i = start;
                    while i < end {
                        let result = step_body(i);
                        if result.should_stop() {
                            loop_result = result;
                            break;
                        }
                        i += step;
                    }
                }
                if matches!(loop_result, ExecResult::Break) {
                    return ExecResult::Continue;
                }
                if loop_result.should_stop() {
                    return loop_result;
                }
            } else {
                for _ in 0..*count {
                    let result = exec(sim, body);
                    if matches!(result, ExecResult::Break) {
                        break;
                    }
                    if result.should_stop() {
                        return result;
                    }
                }
            }
            ExecResult::Continue
        }
        TestbenchStatement::Finish => ExecResult::Finished,
    }
}
