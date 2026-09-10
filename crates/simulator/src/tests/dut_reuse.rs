use super::*;
use crate::backend::inst::{DutReuseCache, ReuseOutcome};
use crate::ir::{BuildSession, ProtoModuleCache};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use veryl_parser::{resource_table, text_table};

const DESIGN: &str = r#"
module Dut (
    clk: input clock,
    rst: input reset,
    i: input logic<32>,
    q: output logic<32>,
    pad: output logic<2048>,
) {
    var sampled: logic<32>;
    always_ff {
        if_reset { sampled = 0; }
        else { sampled = i; }
    }
    assign q = sampled + 7;
    assign pad = 2048'd0;
}
module First (
    clk: input clock,
    rst: input reset,
    i: input logic<32>,
    q: output logic<32>,
    pad: output logic<2048>,
) { inst d: Dut (clk, rst, i, q, pad); }
module Second (
    clk: input clock,
    rst: input reset,
    i: input logic<32>,
    q: output logic<32>,
    pad: output logic<2048>,
) { inst d: Dut (clk, rst, i, q, pad); }
"#;

fn dut_component(ir: &air::Ir) -> Arc<air::Component> {
    ir.components
        .iter()
        .find_map(|c| {
            let air::Component::Module(m) = c else {
                return None;
            };
            m.declarations.iter().find_map(|d| match d {
                air::Declaration::Inst(i) => Some(Arc::clone(&i.component)),
                _ => None,
            })
        })
        .unwrap()
}

#[test]
fn dut_reuse_single_flight_and_abandoned_claims_are_session_local() {
    let ir = analyze_air(DESIGN);
    let component = dut_component(&ir);
    let tops = ["First".into(), "Second".into()];
    let cache = Arc::new(DutReuseCache::new(&ir, &tops));
    let other_cache = Arc::new(DutReuseCache::new(&ir, &tops));
    let key = Arc::as_ptr(&component);
    let ReuseOutcome::Compute(abandoned) = cache.try_reuse_or_claim(key, false, 0, 0, true) else {
        panic!("first claim must compute")
    };
    // A computing slot in one session cannot block a different session using
    // exactly the same live component address.
    let ReuseOutcome::Compute(other) = other_cache.try_reuse_or_claim(key, false, 0, 0, true)
    else {
        panic!("another session must claim independently")
    };
    drop(abandoned);

    let computes = AtomicUsize::new(0);
    let barrier = Barrier::new(8);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let (cache, component, computes, barrier) = (&cache, &component, &computes, &barrier);
            scope.spawn(move || {
                barrier.wait();
                match cache.try_reuse_or_claim(Arc::as_ptr(component), false, 0, 0, true) {
                    ReuseOutcome::Compute(guard) => {
                        computes.fetch_add(1, Ordering::Relaxed);
                        guard.store(
                            0,
                            0,
                            16,
                            24,
                            &crate::HashMap::default(),
                            &[],
                            &[],
                            &[],
                            &[],
                            &[],
                        );
                    }
                    ReuseOutcome::Hit(entry) => {
                        assert_eq!((entry.ff_size, entry.comb_size), (16, 24))
                    }
                    ReuseOutcome::Disabled => panic!("reuse unexpectedly disabled"),
                }
            });
        }
    });
    assert_eq!(
        computes.load(Ordering::Relaxed),
        1,
        "only one worker may compute the DUT"
    );
    drop(other);
    assert!(matches!(
        cache.try_reuse_or_claim(key, false, 0, 0, true),
        ReuseOutcome::Hit(_)
    ));
    assert!(matches!(
        other_cache.try_reuse_or_claim(key, false, 0, 0, true),
        ReuseOutcome::Compute(_)
    ));
}

#[test]
fn dut_reuse_built_ir_outlives_its_session() {
    for mut config in Config::all() {
        config.dut_reuse = true;
        let built = {
            let ir = analyze_air(DESIGN);
            let tops = ["First".into(), "Second".into()];
            let session = BuildSession::new(&ir, &config, &tops);
            drop(session.build_ir(tops[0]).unwrap());
            session.build_ir(tops[1]).unwrap()
        };
        // Dropping the session releases cache entries, but the returned IR
        // must retain every compiled artifact it dispatches.
        assert_eq!(
            observe(built).values,
            vec![7, 8, 12, 49]
                .into_iter()
                .map(|x| Value::new(x, 32, false))
                .collect::<Vec<_>>()
        );
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Observation {
    use_4state: bool,
    aliased_input: bool,
    ff_bytes: usize,
    comb_bytes: usize,
    compiled: usize,
    comb_passes: usize,
    values: Vec<Value>,
}

fn observe(ir: Ir) -> Observation {
    let input_slot = |m: &ModuleVariables| {
        m.variables
            .values()
            .find(|v| v.path.to_string() == "i")
            .unwrap()
            .current_values[0]
    };
    let mut ret = Observation {
        use_4state: ir.use_4state,
        aliased_input: input_slot(&ir.module_variables)
            == input_slot(&ir.module_variables.children[0]),
        ff_bytes: ir.ff_values.len(),
        comb_bytes: ir.comb_values.len(),
        compiled: ir
            .comb_statements
            .iter()
            .chain(ir.event_statements.values().flatten())
            .filter(|s| s.is_compiled())
            .count(),
        comb_passes: ir.required_comb_passes,
        values: vec![],
    };
    let mut sim = Simulator::new(ir, None);
    let clk = sim.get_clock("clk").unwrap();
    let rst = sim.get_reset("rst").unwrap();
    sim.step_reset(&clk, &rst);
    ret.values.push(sim.get("q").unwrap());
    for input in [1, 5, 42] {
        sim.set("i", Value::new(input, 32, false));
        sim.step(&clk);
        ret.values.push(sim.get("q").unwrap());
        assert_eq!(sim.get("pad").unwrap(), Value::new(0, 2048, false));
    }
    ret
}

#[test]
fn dut_reuse_sessions_keep_distinct_top_sets() {
    let ir = analyze_air(DESIGN);
    let tops = ["First".into(), "Second".into()];
    let config = Config {
        dut_reuse: true,
        ..Default::default()
    };
    let shared = BuildSession::new(&ir, &config, &tops);
    // Exactly the same live Arc<Component>, but no recurring DUT in this
    // session. This detects set replacement without relying on address reuse.
    let single = BuildSession::new(&ir, &config, &tops[..1]);
    let isolated = observe(build_ir(&ir, tops[0], &config).unwrap());
    for _ in 0..3 {
        let reused = observe(shared.build_ir(tops[0]).unwrap());
        let unshared = observe(single.build_ir(tops[0]).unwrap());
        assert!(
            !reused.aliased_input,
            "shared DUT must have its own input slot"
        );
        assert!(
            unshared.aliased_input,
            "single-top session must retain port aliasing"
        );
        assert_eq!(unshared, isolated);
        assert_eq!(reused.values, unshared.values);
        assert_eq!(observe(shared.build_ir(tops[1]).unwrap()), reused);
    }
}

#[test]
fn dut_reuse_sessions_isolate_configurations_when_interleaved() {
    let ir = analyze_air(DESIGN);
    let tops = ["First".into(), "Second".into()];
    let configs: Vec<_> = Config::all()
        .into_iter()
        .map(|mut c| {
            c.dut_reuse = true;
            c
        })
        .collect();
    let expected: Vec<_> = configs
        .iter()
        .map(|c| observe(BuildSession::new(&ir, c, &tops).build_ir(tops[0]).unwrap()))
        .collect();
    let sessions: Vec<_> = configs
        .iter()
        .map(|c| BuildSession::new(&ir, c, &tops))
        .collect();
    // Alternate cold and warm caches, including interpreter/JIT, 2/4-state,
    // FF classification and (on hosts with a C compiler) backend selection.
    for top in [tops[0], tops[1], tops[0]] {
        for (session, expected) in sessions.iter().zip(&expected).rev() {
            assert_eq!(&observe(session.build_ir(top).unwrap()), expected);
        }
    }
    assert!(expected.iter().any(|x| x.use_4state));
    assert!(
        expected.windows(2).any(|x| x[0].ff_bytes != x[1].ff_bytes),
        "exercise different FF layouts"
    );
    if !cfg!(target_family = "wasm") {
        assert!(expected.iter().any(|x| x.compiled > 0));
        assert!(expected.iter().any(|x| x.compiled == 0));
    }
}

#[test]
fn dut_reuse_sessions_isolate_configurations_in_parallel() {
    let ir = analyze_air(DESIGN);
    let tops = ["First".into(), "Second".into()];
    let configs: Vec<_> = Config::all()
        .into_iter()
        .map(|mut c| {
            c.dut_reuse = true;
            c
        })
        .collect();
    let expected: Vec<_> = configs
        .iter()
        .map(|c| observe(BuildSession::new(&ir, c, &tops).build_ir(tops[0]).unwrap()))
        .collect();
    let sessions: Vec<_> = configs
        .iter()
        .map(|c| BuildSession::new(&ir, c, &tops))
        .collect();
    let resources = resource_table::export_tables();
    let texts = text_table::export_tables();
    // Two workers per session also exercise shared-DUT single-flight. They
    // race with workers using the same Arc<Component> and different configs.
    let barrier = Barrier::new(2 * sessions.len());
    std::thread::scope(|scope| {
        for (session, expected) in sessions.iter().zip(&expected) {
            for top in tops {
                let (resources, texts, barrier) = (&resources, &texts, &barrier);
                std::thread::Builder::new()
                    .stack_size(crate::IR_WALK_STACK_BYTES)
                    .spawn_scoped(scope, move || {
                        resource_table::import_tables(resources);
                        text_table::import_tables(texts);
                        barrier.wait();
                        for _ in 0..3 {
                            assert_eq!(&observe(session.build_ir(top).unwrap()), expected);
                        }
                    })
                    .unwrap();
            }
        }
    });
}

#[test]
fn dut_reuse_shares_pipeline_only_within_its_session() {
    let ir = analyze_air(DESIGN);
    let tops = ["First".into(), "Second".into()];
    let mut config = Config {
        dut_reuse: true,
        ..Default::default()
    };
    let first_session = BuildSession::new(&ir, &config, &tops);
    let second_session = BuildSession::new(&ir, &config, &tops);
    // The session owns a config snapshot rather than following later edits.
    config.use_4state = true;
    let a = first_session.build_ir(tops[0]).unwrap();
    let b = first_session.build_ir(tops[1]).unwrap();
    let c = second_session.build_ir(tops[0]).unwrap();
    assert!(!a.use_4state);
    assert!(
        Arc::ptr_eq(&a.comb_touched_offsets, &b.comb_touched_offsets),
        "the second top must reuse the comb pipeline"
    );
    assert!(
        !Arc::ptr_eq(&a.comb_touched_offsets, &c.comb_touched_offsets),
        "another session must compute its own pipeline"
    );
    assert_eq!(observe(a), observe(b));
    assert_eq!(
        observe(c).values,
        vec![7, 8, 12, 49]
            .into_iter()
            .map(|x| Value::new(x, 32, false))
            .collect::<Vec<_>>()
    );
}

#[test]
fn dut_reuse_top_caches_are_bound_to_ir_and_config() {
    let first_ir = analyze_air(DESIGN);
    let second_ir = analyze_air(&DESIGN.replace("sampled + 7", "sampled + 19"));
    let tops = ["First".into(), "Second".into()];
    let first_config = Config {
        dut_reuse: true,
        ..Default::default()
    };
    let second_config = Config {
        use_4state: true,
        ..first_config.clone()
    };
    let first_session = BuildSession::new(&first_ir, &first_config, &tops);
    let second_session = BuildSession::new(&second_ir, &second_config, &tops);
    let mut first_cache = ProtoModuleCache::new(&first_session);
    let mut second_cache = ProtoModuleCache::new(&second_session);
    for _ in 0..3 {
        let a = observe(first_cache.build_ir(tops[0]).unwrap());
        let b = observe(second_cache.build_ir(tops[0]).unwrap());
        assert!(!a.use_4state);
        assert!(b.use_4state);
        assert_eq!(
            a.values,
            vec![7, 8, 12, 49]
                .into_iter()
                .map(|x| Value::new(x, 32, false))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            b.values,
            vec![19, 20, 24, 61]
                .into_iter()
                .map(|x| Value::new(x, 32, false))
                .collect::<Vec<_>>()
        );
    }
}
