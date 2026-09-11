use super::*;
use crate::ir::{BuildSession, ProtoModuleCache};
use std::sync::Arc;
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

fn assert_output(ir: Ir, bias: u64) {
    let mut sim = Simulator::new(ir, None);
    let clk = sim.get_clock("clk").unwrap();
    let rst = sim.get_reset("rst").unwrap();
    sim.step_reset(&clk, &rst);
    assert_eq!(sim.get("q").unwrap(), Value::new(bias, 32, false));
    sim.set("i", Value::new(42, 32, false));
    sim.step(&clk);
    assert_eq!(sim.get("q").unwrap(), Value::new(42 + bias, 32, false));
}

fn layout(ir: &Ir) -> (usize, usize, usize) {
    let compiled = ir
        .comb_statements
        .iter()
        .chain(ir.event_statements.values().flatten())
        .filter(|s| s.is_compiled())
        .count();
    (ir.ff_values.len(), ir.comb_values.len(), compiled)
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
    // Creating a second session with the same live component addresses must
    // not replace the first session's recurring set.
    let single = BuildSession::new(&ir, &config, &tops[..1]);
    let input_slot = |m: &ModuleVariables| {
        m.variables
            .values()
            .find(|v| v.path.to_string() == "i")
            .unwrap()
            .current_values[0]
    };
    for (session, aliased) in [(&shared, false), (&single, true), (&shared, false)] {
        let built = session.build_ir(tops[0]).unwrap();
        assert_eq!(
            input_slot(&built.module_variables) == input_slot(&built.module_variables.children[0]),
            aliased
        );
        assert_output(built, 7);
    }
}

#[test]
fn dut_reuse_sessions_isolate_configurations() {
    let ir = analyze_air(DESIGN);
    let tops = ["First".into(), "Second".into()];
    let configs = [
        Config {
            dut_reuse: true,
            ..Default::default()
        },
        Config {
            dut_reuse: true,
            use_4state: true,
            use_jit: !cfg!(target_family = "wasm"),
            disable_ff_opt: true,
            ..Default::default()
        },
    ];
    let expected: Vec<_> = configs
        .iter()
        .map(|c| layout(&BuildSession::new(&ir, c, &tops).build_ir(tops[0]).unwrap()))
        .collect();
    assert_ne!(expected[0], expected[1], "exercise incompatible layouts");
    let resources = resource_table::export_tables();
    let texts = text_table::export_tables();

    for parallel in [false, true] {
        let sessions: Vec<_> = configs
            .iter()
            .map(|c| BuildSession::new(&ir, c, &tops))
            .collect();
        // Seed one session before either worker runs. Both sessions borrow
        // the same live Arc<Component>, so a cache-key collision is certain
        // if their DUT caches are accidentally shared, regardless of timing.
        assert_output(sessions[0].build_ir(tops[0]).unwrap(), 7);
        let check = |index: usize| {
            let built = sessions[index].build_ir(tops[1]).unwrap();
            assert_eq!(layout(&built), expected[index]);
            assert_output(built, 7);
        };
        if parallel {
            std::thread::scope(|scope| {
                for index in 0..2 {
                    let (resources, texts, check) = (&resources, &texts, &check);
                    std::thread::Builder::new()
                        .stack_size(crate::IR_WALK_STACK_BYTES)
                        .spawn_scoped(scope, move || {
                            resource_table::import_tables(resources);
                            text_table::import_tables(texts);
                            check(index);
                        })
                        .unwrap();
                }
            });
        } else {
            check(1);
            check(0);
        }
    }
}

#[test]
fn dut_reuse_shares_pipeline_only_within_its_session() {
    let (a, b, c) = {
        let ir = analyze_air(DESIGN);
        let tops = ["First".into(), "Second".into()];
        let mut config = Config {
            dut_reuse: true,
            use_jit: !cfg!(target_family = "wasm"),
            ..Default::default()
        };
        let first = BuildSession::new(&ir, &config, &tops);
        let second = BuildSession::new(&ir, &config, &tops);
        config.use_4state = true;
        let a = first.build_ir(tops[0]).unwrap();
        let b = first.build_ir(tops[1]).unwrap();
        let c = second.build_ir(tops[0]).unwrap();
        assert!(!a.use_4state, "the session must snapshot its configuration");
        assert!(Arc::ptr_eq(
            &a.comb_touched_offsets,
            &b.comb_touched_offsets
        ));
        assert!(!Arc::ptr_eq(
            &a.comb_touched_offsets,
            &c.comb_touched_offsets
        ));
        (a, b, c)
    };
    // Returned models retain shared artifacts after their sessions and IR drop.
    for built in [a, b, c] {
        assert_output(built, 7);
    }
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
    // Cold and warm top-cache hits use identical names from different IRs.
    for _ in 0..2 {
        let a = first_cache.build_ir(tops[0]).unwrap();
        let b = second_cache.build_ir(tops[0]).unwrap();
        assert!(!a.use_4state);
        assert!(b.use_4state);
        assert_output(a, 7);
        assert_output(b, 19);
    }
}
