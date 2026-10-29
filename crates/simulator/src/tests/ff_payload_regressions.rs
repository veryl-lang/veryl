use super::*;

#[test]
fn interpreted_narrow_ff_log_preserves_four_state_payload() {
    let code = r#"
    module Top (
        clk: input clock, rst: input reset, data: input logic<8>,
        out: output logic<8>, previous: output logic<8>,
    ) {
        var q: logic<8>;
        var prev: logic<8>;
        always_ff {
            if_reset {q = 0; prev = 0;}
            else {q = data; prev = q;}
        }
        assign out = q;
        assign previous = prev;
    }
    "#;
    for disable_ff_opt in [false, true] {
        let config = Config {
            use_4state: true,
            use_jit: false,
            disable_ff_opt,
            ..Default::default()
        };
        let mut sim = Simulator::new(analyze(code, &config), None);
        let clock = sim.get_clock("clk").unwrap();
        let reset = sim.get_reset("rst").unwrap();
        sim.step_reset(&clock, &reset);
        let mut old = Value::new(0, 8, false);
        for literal in ["8'b1z0x1011", "8'bzzzzzzzz", "8'b10101010", "8'bx1x1x1x1"] {
            let data = Value::from_str(literal).unwrap();
            sim.set("data", data.clone());
            sim.step(&clock);
            assert_eq!(sim.get("out"), Some(data.clone()));
            assert_eq!(sim.get("previous"), Some(old));
            old = data;
        }
    }
}

#[test]
fn array_read_in_always_ff_and_at_a_runtime_index() {
    let code = r#"
    module Top (
        clk: input clock, rst: input reset, idx: input logic, i_data: input logic<8>,
        out: output logic<8>,
    ) {
        var regs: logic<8> [2];
        var m: logic<8>;
        always_ff {
            if_reset {regs[0] = 0; regs[1] = 0; m = 0;}
            else {regs[1] = i_data; m = regs[0];}
        }
        assign out = m + regs[idx];
    }
    "#;
    for config in Config::all() {
        let mut sim = Simulator::new(analyze(code, &config), None);
        let clock = sim.get_clock("clk").unwrap();
        let reset = sim.get_reset("rst").unwrap();
        sim.step_reset(&clock, &reset);
        sim.set("idx", Value::new(1, 1, false));
        sim.set("i_data", Value::new(7, 8, false));
        sim.step(&clock);
        assert_eq!(sim.get("out"), Some(Value::new(7, 8, false)), "{config:?}");
    }
}
