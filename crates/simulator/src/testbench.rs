use crate::HashMap;
use crate::ir::{
    Event, Expression, Ir, ModuleVariables, Statement, SystemFunctionCall, TbMethodKind, Value,
    VarId, VarPath,
};
use crate::simulator::Simulator;
use crate::simulator_error::SimulatorError;
use crate::wave_dumper::WaveDumper;
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
    },
    /// $finish
    Finish,
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
            ExecResult::Continue | ExecResult::Finished => TestResult::Pass,
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

/// Build a mapping from $tb instance names (StrId) to their corresponding Events.
///
/// Scans event_statements for TbMethodCall entries to determine which instance
/// corresponds to clock_gen (Event::Clock) or reset_gen (Event::Reset).
/// When no DUT-driven clock/reset events exist (e.g., purely combinational DUT),
/// the actual VarId is resolved from module_variables so that VCD dump works.
pub fn build_event_map(
    event_statements: &HashMap<Event, Vec<Statement>>,
    module_variables: &ModuleVariables,
) -> HashMap<StrId, Event> {
    let clk_event = event_statements
        .keys()
        .find(|e| matches!(e, Event::Clock(_)))
        .cloned();
    let rst_event = event_statements
        .keys()
        .find(|e| matches!(e, Event::Reset(_)))
        .cloned();

    let mut event_map = HashMap::default();
    let mut clock_insts = Vec::new();
    let mut reset_insts = Vec::new();
    for stmts in event_statements.values() {
        for stmt in stmts {
            if let Statement::TbMethodCall { inst, method } = stmt {
                match method {
                    TbMethodKind::ClockNext { .. } => {
                        if let Some(ref evt) = clk_event {
                            event_map.entry(*inst).or_insert(evt.clone());
                        } else {
                            clock_insts.push(*inst);
                        }
                    }
                    TbMethodKind::ResetAssert { .. } => {
                        if let Some(ref evt) = rst_event {
                            event_map.entry(*inst).or_insert(evt.clone());
                        } else {
                            reset_insts.push(*inst);
                        }
                    }
                }
            }
        }
    }

    if clk_event.is_none() && !clock_insts.is_empty() {
        for inst in clock_insts {
            let var_id = find_var_id_by_name(module_variables, inst).unwrap_or(VarId::SYNTHETIC);
            event_map.entry(inst).or_insert(Event::Clock(var_id));
        }
    }
    if rst_event.is_none() && !reset_insts.is_empty() {
        for inst in reset_insts {
            let var_id = find_var_id_by_name(module_variables, inst).unwrap_or(VarId::SYNTHETIC);
            event_map.entry(inst).or_insert(Event::Reset(var_id));
        }
    }

    event_map
}

/// period < 2 is clamped to 2. Remainder goes to high (posedge) phase.
fn compute_half_periods(period: u64) -> (u64, u64) {
    let p = period.max(2);
    (p.div_ceil(2), p / 2)
}

pub fn build_clock_periods(
    event_statements: &HashMap<Event, Vec<Statement>>,
) -> HashMap<StrId, u64> {
    let mut periods = HashMap::default();
    for stmts in event_statements.values() {
        for stmt in stmts {
            if let Statement::TbMethodCall { inst, method } = stmt
                && let TbMethodKind::ClockNext { period, .. } = method
                && let Some(expr) = period
            {
                let val = expr.eval(&mut MaskCache::default());
                periods.entry(*inst).or_insert(val.payload_u64());
            }
        }
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
/// Builds a Simulator, extracts the initial block, converts it to testbench
/// statements, and executes them.
pub fn run_native_testbench(
    ir: Ir,
    dump: Option<WaveDumper>,
) -> Result<TestResult, SimulatorError> {
    let mut sim = Simulator::new(ir, dump);
    let event_map = build_event_map(&sim.ir.event_statements, &sim.ir.module_variables);
    let clock_periods = build_clock_periods(&sim.ir.event_statements);

    let module_name = sim.ir.name.to_string();
    let token = sim.ir.token;
    let initial_stmts = sim
        .ir
        .event_statements
        .get(&Event::Initial)
        .ok_or_else(|| SimulatorError::no_initial_block(&module_name, &token))?;

    let tb_stmts = convert_initial_to_testbench(initial_stmts, &event_map, &clock_periods, 3);
    Ok(run_testbench(&mut sim, &tb_stmts))
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
            s.eval_step(&mut sim.mask_cache);
            sim.mark_comb_dirty();
            ExecResult::Continue
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
            for _ in 0..n {
                if has_dump && let Some(id) = clock.var_id() {
                    sim.set_var_by_id(&id, Value::new(1, 1, false));
                }
                sim.step(clock);
                sim.time += high_time;
                if has_dump {
                    if let Some(id) = clock.var_id() {
                        sim.set_var_by_id(&id, Value::new(0, 1, false));
                    }
                    sim.dump_variables();
                }
                sim.time += low_time;
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
        TestbenchStatement::For { count, body } => {
            for _ in 0..*count {
                let result = exec(sim, body);
                if result.should_stop() {
                    return result;
                }
            }
            ExecResult::Continue
        }
        TestbenchStatement::Finish => ExecResult::Finished,
    }
}
