//! Runtime driving of user-defined components: builds instances from the
//! IR's `external_components`, stages pre-edge inputs before FF commit,
//! fires hooks after it, and writes dirty outputs back into variable
//! storage (NBA-equivalent: the same edge's RTL never observes them).

use crate::HashMap;
use crate::component::host::{ExternalInstance, HostContext, HostValue, PortDir, PortRole};
use crate::component::loader::{ComponentError, lookup_component_backend};
use crate::ir::ModuleVariables;
use crate::ir::{Event, Expression, Ir, VarId};
use num_bigint::BigUint;
use veryl_analyzer::value::{MaskCache, Value, ValueBigUint, ValueU64};
use veryl_component_sys as sys;
use veryl_metadata::{
    ComponentManifest,
    component_manifest::{ConnectionFacts, ConnectionTarget, ConnectionViolation},
};
use veryl_parser::resource_table;
use veryl_parser::resource_table::StrId;

pub struct RuntimeComponent {
    pub name: String,
    pub name_id: resource_table::StrId,
    pub instance: ExternalInstance,
    pub host: HostContext,
    /// Whether the manifest declares `requires(file)`; `None` without a
    /// manifest. Native runs never block file access, so undeclared use is
    /// surfaced as a warning to catch it before the wasm form refuses it.
    file_declared: Option<bool>,
    /// Inputs staged before every hook: (host port idx, expression, width).
    inputs: Vec<(u32, Expression, u32)>,
    /// Dirty outputs written back after hooks: (host port idx, destination).
    outputs: Vec<(u32, VarId)>,
    /// Host input-port index of each connected clock/reset event, for
    /// `fired_clock` and the event maps.
    pub clock_events: Vec<(Event, u32)>,
    pub reset_events: Vec<(Event, u32)>,
    /// Per-instance count of fired clock hooks, reported as `ctx.cycle()`.
    fire_count: u64,
    /// Reused by `stage_inputs` so staging allocates no words per port/step.
    words_scratch: Vec<u64>,
    mask_scratch: Vec<u64>,
}

fn str_of(id: resource_table::StrId) -> String {
    resource_table::get_str_value(id).unwrap_or_default()
}

/// Deterministic FNV-1a over the test seed, test name and instance name.
fn instance_seed(base: u64, test_name: &str, instance: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    eat(&base.to_le_bytes());
    eat(test_name.as_bytes());
    eat(instance.as_bytes());
    h
}

/// Payload words of a value, LSB-first.
fn value_to_words(value: &Value, nwords: usize) -> Vec<u64> {
    let mut words = Vec::new();
    value_to_words_into(value, nwords, &mut words);
    words
}

/// Payload words of a value into a reused buffer, LSB-first.
fn value_to_words_into(value: &Value, nwords: usize, out: &mut Vec<u64>) {
    out.clear();
    match value {
        Value::U64(x) => out.push(x.payload),
        Value::BigUint(x) => out.extend(x.payload.iter_u64_digits()),
    }
    out.resize(nwords, 0);
}

/// Four-state mask words of a value (X/Z bits) into a reused buffer, LSB-first.
fn value_to_mask_xz_into(value: &Value, nwords: usize, out: &mut Vec<u64>) {
    out.clear();
    match value {
        Value::U64(x) => out.push(x.mask_xz),
        Value::BigUint(x) => out.extend(x.mask_xz.iter_u64_digits()),
    }
    out.resize(nwords, 0);
}

pub fn host_value_from(value: &Value) -> HostValue {
    let width = value.width() as u32;
    let words = value_to_words(value, (width as usize).div_ceil(64).max(1));
    HostValue::Bits { words, width }
}

/// `None` for unit (no value) and strings.
pub fn host_value_to_value(value: &HostValue) -> Option<Value> {
    match value {
        HostValue::Bits { words, width } => Some(words_to_value(words, *width)),
        _ => None,
    }
}

pub(crate) fn words_to_value(words: &[u64], width: u32) -> Value {
    words_to_value_masked(words, &[], width)
}

/// Builds a value from payload `words` and a parallel four-state `mask_xz` (X/Z
/// bits); an empty `mask_xz` yields a two-state value.
pub(crate) fn words_to_value_masked(words: &[u64], mask_xz: &[u64], width: u32) -> Value {
    let to_big = |ws: &[u64]| -> BigUint {
        let bytes: Vec<u8> = ws.iter().flat_map(|w| w.to_le_bytes()).collect();
        BigUint::from_bytes_le(&bytes)
    };
    if width <= 64 {
        Value::U64(ValueU64 {
            payload: words.first().copied().unwrap_or(0),
            mask_xz: mask_xz.first().copied().unwrap_or(0),
            width,
            signed: false,
        })
    } else {
        Value::BigUint(ValueBigUint {
            payload: Box::new(to_big(words)),
            mask_xz: Box::new(to_big(mask_xz)),
            width,
            signed: false,
        })
    }
}

/// What a connection binds to; see
/// [`ComponentManifest::connection_target`] for the binding contract
/// shared with the analyzer.
fn connection_target_of<'a>(
    manifest: &'a ComponentManifest,
    connect: &crate::ir::ExternalConnectInst,
) -> Option<ConnectionTarget<'a>> {
    let group = connect.group.map(str_of);
    let member = connect.member.map(str_of);
    manifest.connection_target(&str_of(connect.port), group.as_deref(), member.as_deref())
}

/// Checks an instance's connections and parameters against the declared
/// manifest. Modport group members absent from the manifest are tolerated,
/// mirroring the unused-member tolerance of the group check.
fn check_manifest_connections(
    ext: &crate::ir::ExternalComponentInst,
    manifest: &ComponentManifest,
    inst_name: &str,
    type_name: &str,
) -> Result<(), ComponentError> {
    let mk = |port: &str| {
        (
            inst_name.to_string(),
            type_name.to_string(),
            port.to_string(),
        )
    };
    let has_decls = !(manifest.ports.is_empty() && manifest.groups.is_empty());
    // Group connections must name declared groups. Mirrors the
    // analyzer-side check.
    if has_decls {
        let mut seen_groups: std::collections::HashSet<StrId> = std::collections::HashSet::new();
        for connect in &ext.connects {
            let Some(group) = connect.group else {
                continue;
            };
            if seen_groups.contains(&group) {
                continue;
            }
            if !manifest.groups.iter().any(|g| g.name == str_of(group)) {
                return Err(ComponentError::UnknownGroup {
                    inst: inst_name.to_string(),
                    type_name: type_name.to_string(),
                    group: str_of(group),
                });
            }
            seen_groups.insert(group);
        }
    }
    for grp in &manifest.groups {
        if grp.members.is_empty() || grp.members.iter().any(|m| m.member.is_empty()) {
            return Err(ComponentError::ManifestInvalid {
                inst: inst_name.to_string(),
                reason: format!("interface port `{}` declares no valid members", grp.name),
            });
        }
    }
    let mut bound: std::collections::HashSet<String> = std::collections::HashSet::new();
    for connect in &ext.connects {
        if !has_decls {
            break;
        }
        let port_name = str_of(connect.port);
        let Some(target) = connection_target_of(manifest, connect) else {
            if connect.group.is_none() {
                let (inst, type_name, port) = mk(&port_name);
                return Err(ComponentError::UnknownPort {
                    inst,
                    type_name,
                    port,
                });
            }
            continue;
        };
        // A port bound twice would race for one host port; for members
        // this catches the same group connected more than once.
        let display = match &target {
            ConnectionTarget::Loose(port) => port.name.clone(),
            ConnectionTarget::Member(..) => port_name.clone(),
        };
        if !bound.insert(display.clone()) {
            let (inst, type_name, port) = mk(&display);
            return Err(ComponentError::PortMultiplyConnected {
                inst,
                type_name,
                port,
            });
        }
        // Port widths are inferred from the connection; a component validates
        // any width constraints itself in `on_build`.
        let facts = ConnectionFacts {
            input: connect.input,
            drivable: connect.output.is_some(),
            is_clock: connect.is_clock,
            is_reset: connect.is_reset,
        };
        if let Some(violation) = target.check(&facts).into_iter().next() {
            let (inst, type_name, port) = mk(&display);
            return Err(match violation {
                ConnectionViolation::InvalidDirection(dir) => {
                    ComponentError::InvalidPortDirection {
                        inst,
                        type_name,
                        port,
                        dir,
                    }
                }
                ConnectionViolation::NotInput => ComponentError::PortNotInput {
                    inst,
                    type_name,
                    port,
                },
                ConnectionViolation::NotDrivable => ComponentError::PortNotDrivable {
                    inst,
                    type_name,
                    port,
                },
                ConnectionViolation::NotClock => ComponentError::PortNotClock {
                    inst,
                    type_name,
                    port,
                },
                ConnectionViolation::NotReset => ComponentError::PortNotReset {
                    inst,
                    type_name,
                    port,
                },
                ConnectionViolation::ClockUndeclared => ComponentError::ClockPortUndeclared {
                    inst,
                    type_name,
                    port,
                },
                ConnectionViolation::ResetUndeclared => ComponentError::ResetPortUndeclared {
                    inst,
                    type_name,
                    port,
                },
            });
        }
    }
    // Clock/reset ports are resolved unconditionally by the component, so
    // report a missing connection by name before `create` fails opaquely.
    for port in &manifest.ports {
        let Some(role) = port.role.as_deref() else {
            continue;
        };
        if !bound.contains(&port.name) {
            return Err(ComponentError::RolePortUnconnected {
                inst: inst_name.to_string(),
                type_name: type_name.to_string(),
                port: port.name.clone(),
                role: role.to_string(),
            });
        }
    }
    for (name, _) in &ext.params {
        let param_name = str_of(*name);
        if !manifest.params.is_empty() && manifest.param(&param_name).is_none() {
            return Err(ComponentError::UnknownParam {
                inst: inst_name.to_string(),
                type_name: type_name.to_string(),
                param: param_name,
            });
        }
    }
    Ok(())
}

/// Builds every component instance for the IR: resolves vtables, sets up
/// ports/params, runs `create`, and validates the connection contract.
/// Errors carry the instance name and become `TestResult::Fail`.
pub fn build_components(
    ir: &Ir,
    seed_base: u64,
    test_name: &str,
) -> Result<Vec<RuntimeComponent>, ComponentError> {
    // Sole-driver check: destination variable -> driving instance name.
    let mut driven: HashMap<VarId, String> = HashMap::default();
    let mut result = vec![];
    for ext in &ir.external_components {
        let inst_name = str_of(ext.name);
        let component_name = str_of(ext.component);
        // `[[components]]` entries built by `veryl test` resolve to a dynamic
        // library; anything else (unit tests, builtins) uses the static
        // registry under the component name itself.
        let (library, type_name) = match ir.component_libraries.get(&component_name) {
            Some(lib) => (Some(lib.path.as_path()), lib.type_name.clone()),
            None => (None, component_name.clone()),
        };
        let in_instance = |e: ComponentError| ComponentError::Instance {
            inst: inst_name.clone(),
            source: Box::new(e),
        };
        let backend = lookup_component_backend(library, &type_name).map_err(&in_instance)?;
        let kind = backend.kind();

        // Optional declared interface from `#[component]`: connections and
        // parameters are checked against it before `create` runs. A
        // manifest that exists but cannot be parsed is a load error; only
        // its absence skips the checks.
        let manifest = match library {
            Some(path) => match crate::component::loader::library_manifest(path) {
                Some(json) => {
                    crate::component::loader::parse_library_manifest_json(&json, &type_name)
                        .map_err(|reason| ComponentError::ManifestInvalid {
                            inst: inst_name.clone(),
                            reason,
                        })?
                }
                None => None,
            },
            None => match crate::component::loader::static_manifest(&type_name) {
                Some(json) => Some(ComponentManifest::parse(&json).ok_or_else(|| {
                    ComponentError::ManifestParse {
                        inst: inst_name.clone(),
                        type_name: type_name.clone(),
                    }
                })?),
                None => None,
            },
        };
        if let Some(manifest) = &manifest {
            check_manifest_connections(ext, manifest, &inst_name, &type_name)?;
        }
        if ext.is_var_form {
            if kind == sys::VRL_KIND_CLOCKED {
                return Err(ComponentError::ClockedNeedsInst {
                    inst: inst_name.clone(),
                    type_name: type_name.clone(),
                });
            }
        } else if kind == sys::VRL_KIND_METHOD_ONLY {
            return Err(ComponentError::MethodOnlyNeedsVar {
                inst: inst_name.clone(),
                type_name: type_name.clone(),
            });
        }

        let mut host = HostContext::new();
        host.label = inst_name.clone();
        host.use_4state = ir.use_4state;
        host.seed = instance_seed(seed_base, test_name, &inst_name);
        if let Some(base) = &ir.component_file_base {
            host.read_base = Some(base.clone());
            host.write_base = Some(
                base.join("target")
                    .join("veryl-components")
                    .join("out")
                    .join(test_name),
            );
        }

        let mut inputs = vec![];
        let mut outputs = vec![];
        let mut clock_events = vec![];
        let mut reset_events = vec![];
        // (port name, group, input idx, output idx) for the
        // unused-connection check
        let mut connection_ports = vec![];
        // Directly-connected clock/reset ports the component must resolve
        // with the matching role; a modport group member may go unused.
        let mut role_connections = vec![];

        for connect in &ext.connects {
            let port_name = str_of(connect.port);
            if connect.width == 0 {
                return Err(ComponentError::UndeterminedWidth {
                    inst: inst_name.clone(),
                    port: port_name,
                });
            }
            let in_idx = connect.input.then(|| {
                let role = if connect.is_clock {
                    PortRole::Clock
                } else if connect.is_reset {
                    PortRole::Reset
                } else {
                    PortRole::Data
                };
                let in_idx = host.add_port_role(&port_name, PortDir::Input, role, connect.width);
                inputs.push((in_idx, connect.expr.clone(), connect.width));
                in_idx
            });
            let out_idx = connect.output.map(|var_id| {
                let out_idx = host.add_port(&port_name, PortDir::Output, connect.width);
                outputs.push((out_idx, var_id));
                out_idx
            });
            // A clock/reset input without an event source would never
            // fire and pass vacuously; reject it instead.
            if (connect.is_clock || connect.is_reset)
                && connect.input
                && connect.event_var.is_none()
            {
                let role = if connect.is_clock { "clock" } else { "reset" };
                return Err(ComponentError::UndeterminedEventSource {
                    inst: inst_name.clone(),
                    port: port_name.clone(),
                    role: role.to_string(),
                });
            }
            if let Some(in_idx) = in_idx
                && connect.group.is_none()
            {
                if connect.is_clock {
                    role_connections.push((port_name.clone(), in_idx, PortRole::Clock));
                }
                if connect.is_reset {
                    role_connections.push((port_name.clone(), in_idx, PortRole::Reset));
                }
            }
            connection_ports.push((port_name, connect.group, in_idx, out_idx));

            // Firing needs the input port index (`fired_clock`), so an
            // event is only usable on an input-capable connection. Whether
            // the event actually fires the component is decided after
            // `create`, from how it resolved the port.
            if let (Some(var_id), Some(in_idx)) = (connect.event_var, in_idx) {
                if connect.is_clock {
                    clock_events.push((Event::Clock(var_id), in_idx));
                }
                if connect.is_reset {
                    reset_events.push((Event::Reset(var_id), in_idx));
                }
            }
        }

        for (name, value) in &ext.params {
            let host_value = match value {
                veryl_analyzer::ir::ExternalParamValue::Value(value) => host_value_from(value),
                veryl_analyzer::ir::ExternalParamValue::Str(s) => HostValue::Str(s.clone()),
            };
            host.add_param(&str_of(*name), host_value);
        }

        let instance = ExternalInstance::create(backend, &mut host).map_err(&in_instance)?;

        // Hooks fire only on ports the component resolved as clock/reset
        // (`BuildCtx::clock`/`reset`); a clock connection read as plain
        // data stays a data input. A directly-connected clock/reset the
        // component did not resolve as such would silently never fire its
        // hook, so it is rejected.
        for (port_name, in_idx, role) in &role_connections {
            if host.port_resolved_role(*in_idx) != Some(*role) {
                let (role_str, port_ty) = match role {
                    PortRole::Clock => ("clock", "ClockPort"),
                    _ => ("reset", "ResetPort"),
                };
                return Err(ComponentError::RoleNotResolved {
                    inst: inst_name.clone(),
                    type_name: type_name.clone(),
                    port: port_name.clone(),
                    role: role_str.to_string(),
                    field: port_ty.to_string(),
                });
            }
        }
        clock_events
            .retain(|(_, in_idx)| host.port_resolved_role(*in_idx) == Some(PortRole::Clock));
        reset_events
            .retain(|(_, in_idx)| host.port_resolved_role(*in_idx) == Some(PortRole::Reset));

        if kind == sys::VRL_KIND_CLOCKED && clock_events.is_empty() {
            return Err(ComponentError::NoClockPortResolved {
                inst: inst_name.clone(),
                type_name: type_name.clone(),
            });
        }

        // Self-consistency: a port the component resolved but did not
        // declare (or declared with the other direction) is a component
        // bug — its manifest is out of date. Warn, don't fail.
        if let Some(manifest) = &manifest {
            for (port_name, dir) in host.touched_port_names() {
                let dir_str = match dir {
                    PortDir::Input => "input",
                    PortDir::Output => "output",
                };
                let target = ext
                    .connects
                    .iter()
                    .find(|c| str_of(c.port) == port_name)
                    .and_then(|c| connection_target_of(manifest, c));
                match target {
                    None => log::warn!(
                        "component `{inst_name}`: `{type_name}` uses port `{port_name}` which its manifest does not declare"
                    ),
                    Some(t) if t.dir() != dir_str => log::warn!(
                        "component `{inst_name}`: `{type_name}` uses port `{port_name}` as an {dir_str} but its manifest declares an {}",
                        t.dir()
                    ),
                    Some(_) => {}
                }
            }
        }

        // Ungrouped connections must each be used; a modport group only
        // needs one used member (a component rarely uses every signal).
        let mut group_order = vec![];
        let mut group_used: HashMap<resource_table::StrId, bool> = HashMap::default();
        for (port_name, group, in_idx, out_idx) in connection_ports {
            let used = in_idx.is_some_and(|i| host.port_touched(i))
                || out_idx.is_some_and(|i| host.port_touched(i));
            match group {
                Some(group) => {
                    let entry = group_used.entry(group).or_insert_with(|| {
                        group_order.push(group);
                        false
                    });
                    *entry |= used;
                }
                None => {
                    if !used {
                        return Err(ComponentError::PortUnused {
                            inst: inst_name.clone(),
                            type_name: type_name.clone(),
                            port: port_name,
                        });
                    }
                }
            }
        }
        for group in group_order {
            if !group_used[&group] {
                return Err(ComponentError::GroupUnused {
                    inst: inst_name.clone(),
                    type_name: type_name.clone(),
                    group: str_of(group),
                });
            }
        }
        // Output ports the component asked for keep only those bindings.
        outputs.retain(|(idx, _)| host.port_touched(*idx));
        inputs.retain(|(idx, _, _)| host.port_touched(*idx));

        // A component output must be the destination's only driver.
        for (_, var_id) in &outputs {
            let var_name = ir
                .module_variables
                .variables
                .get(var_id)
                .map(|v| v.path.to_string())
                .unwrap_or_default();
            if ir.rtl_driven.contains(var_id) {
                return Err(ComponentError::OutputRtlConflict {
                    inst: inst_name.clone(),
                    var: var_name,
                });
            }
            if let Some(other) = driven.insert(*var_id, inst_name.clone()) {
                return Err(ComponentError::OutputComponentConflict {
                    inst: inst_name.clone(),
                    var: var_name,
                    other,
                });
            }
        }

        let file_declared = manifest
            .as_ref()
            .map(|m| m.requires.iter().any(|r| r == "file"));
        result.push(RuntimeComponent {
            name: inst_name,
            name_id: ext.name,
            instance,
            host,
            file_declared,
            inputs,
            outputs,
            clock_events,
            reset_events,
            fire_count: 0,
            words_scratch: Vec::new(),
            mask_scratch: Vec::new(),
        });
    }
    Ok(result)
}

impl RuntimeComponent {
    /// Moves accumulated `ctx.log` messages into the per-test output
    /// buffer (joining `$display` output).
    pub fn drain_logs(&mut self) {
        for msg in self.host.take_logs() {
            crate::output_buffer::println(&msg);
        }
    }

    /// Evaluates every input connection (pre-commit, i.e. pre-edge values)
    /// into the host staging buffers. In two-state mode the X/Z mask is
    /// always zero, so its per-step computation and copy are skipped.
    pub fn stage_inputs(&mut self, mask_cache: &mut MaskCache) {
        let use_4state = self.host.use_4state;
        for (idx, expr, width) in &self.inputs {
            let value = expr.eval(mask_cache);
            let nwords = (*width as usize).div_ceil(64).max(1);
            value_to_words_into(&value, nwords, &mut self.words_scratch);
            if use_4state {
                value_to_mask_xz_into(&value, nwords, &mut self.mask_scratch);
                self.host
                    .set_input_masked(*idx, &self.words_scratch, &self.mask_scratch);
            } else {
                self.host.set_input(*idx, &self.words_scratch);
            }
        }
    }

    /// Runs one hook; a non-zero return code is a component failure.
    /// Well-behaved guests report the cause via `fail` before returning
    /// it; fall back to a generic message naming the hook.
    fn run_hook(&mut self, hook: &str, run: fn(&mut ExternalInstance, &mut HostContext) -> i32) {
        let failures_before = self.host.failures().len();
        let rc = run(&mut self.instance, &mut self.host);
        if rc != 0 && self.host.failures().len() == failures_before {
            self.host
                .svc_fail(&format!("component hook `{hook}` failed"));
        }
    }

    /// Fires `on_clock` (optionally preceded by `on_reset`) for one event.
    pub fn fire(&mut self, event: &Event, time: u64) {
        self.host.time = time;
        match event {
            Event::Reset(_) => {
                let in_idx = self
                    .reset_events
                    .iter()
                    .find(|(e, _)| e == event)
                    .map(|(_, i)| *i);
                if let Some(in_idx) = in_idx {
                    self.host.fired_clock = in_idx;
                    self.run_hook("on_reset", |i, h| i.on_reset(h));
                }
            }
            Event::Clock(_) => {
                let in_idx = self
                    .clock_events
                    .iter()
                    .find(|(e, _)| e == event)
                    .map(|(_, i)| *i);
                if let Some(in_idx) = in_idx {
                    self.fire_count += 1;
                    self.host.cycle = self.fire_count;
                    self.host.fired_clock = in_idx;
                    self.run_hook("on_clock", |i, h| i.on_clock(h));
                }
            }
            _ => {}
        }
    }

    pub fn on_init(&mut self) {
        self.run_hook("on_init", |i, h| i.on_init(h));
    }

    pub fn on_finish(&mut self) {
        self.run_hook("on_finish", |i, h| i.on_finish(h));
        if self.file_declared == Some(false) && !self.host.touched_files.is_empty() {
            log::warn!(
                "component `{}`: uses the host file service but its manifest does not declare `requires(file)`; the prebuilt wasm form will refuse file access",
                self.name
            );
        }
    }

    pub fn listens_to(&self, event: &Event) -> bool {
        match event {
            Event::Clock(_) => self.clock_events.iter().any(|(e, _)| e == event),
            Event::Reset(_) => self.reset_events.iter().any(|(e, _)| e == event),
            _ => false,
        }
    }

    /// Writes dirty outputs into variable storage. Runs after FF commit so
    /// the same edge's RTL saw pre-edge values (NBA semantics). Returns
    /// true when anything was written (the caller marks comb dirty).
    pub fn apply_outputs(&mut self, variables: &mut ModuleVariables, use_4state: bool) -> bool {
        let mut wrote = false;
        for (idx, var_id) in &self.outputs {
            if !self.host.output_dirty_idx(*idx) {
                continue;
            }
            let Some(var) = variables.variables.get_mut(var_id) else {
                continue;
            };
            // Two-state mode carries no X/Z, so skip the mask read (and the
            // wide-value mask reconstruction it would drive).
            let mask_xz = if use_4state {
                self.host.output_mask_xz_idx(*idx)
            } else {
                &[]
            };
            let mut value =
                words_to_value_masked(self.host.output_words_idx(*idx), mask_xz, var.width as u32);
            value.trunc(var.width);
            unsafe {
                crate::ir::write_native_value(
                    var.current_values[0],
                    var.native_bytes,
                    use_4state,
                    &value,
                );
            }
            wrote = true;
        }
        self.host.clear_output_dirty();
        wrote
    }
}
