use super::*;

#[test]
fn testbench_counter_clock_next() {
    let code = r#"
    module Top (
        clk: input clock,
        rst: input reset,
        cnt: output logic<32>,
    ) {
        always_ff {
            if_reset { cnt = 0; }
            else { cnt += 1; }
        }
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        let stmts = vec![
            TestbenchStatement::ResetAssert {
                reset: rst.clone(),
                clock: clk.clone(),
                duration: 3,
                high_time: 1,
                low_time: 1,
            },
            TestbenchStatement::For {
                count: 10,
                body: vec![TestbenchStatement::ClockNext {
                    clock: clk.clone(),
                    count: None,
                    high_time: 1,
                    low_time: 1,
                }],
                loop_var: None,
            },
            TestbenchStatement::Finish,
        ];

        let result = run_testbench(&mut sim, &stmts);
        assert_eq!(result, TestResult::Pass);
        assert_eq!(sim.get("cnt").unwrap(), Value::new(10, 32, false));
    }
}

#[test]
fn testbench_reset_clears_counter() {
    let code = r#"
    module Top (
        clk: input clock,
        rst: input reset,
        cnt: output logic<32>,
    ) {
        always_ff {
            if_reset { cnt = 0; }
            else { cnt += 1; }
        }
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        let stmts = vec![
            TestbenchStatement::ResetAssert {
                reset: rst.clone(),
                clock: clk.clone(),
                duration: 5,
                high_time: 1,
                low_time: 1,
            },
            TestbenchStatement::Finish,
        ];

        let result = run_testbench(&mut sim, &stmts);
        assert_eq!(result, TestResult::Pass);
        assert_eq!(sim.get("cnt").unwrap(), Value::new(0, 32, false));
    }
}

#[test]
fn testbench_for_loop() {
    let code = r#"
    module Top (
        clk: input clock,
        rst: input reset,
        cnt: output logic<32>,
    ) {
        always_ff {
            if_reset { cnt = 0; }
            else { cnt += 1; }
        }
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        let stmts = vec![
            TestbenchStatement::ResetAssert {
                reset: rst.clone(),
                clock: clk.clone(),
                duration: 1,
                high_time: 1,
                low_time: 1,
            },
            // Step 5 times using For loop (each iteration steps 1 clock)
            TestbenchStatement::For {
                count: 5,
                body: vec![TestbenchStatement::ClockNext {
                    clock: clk.clone(),
                    count: None,
                    high_time: 1,
                    low_time: 1,
                }],
                loop_var: None,
            },
            TestbenchStatement::Finish,
        ];

        let result = run_testbench(&mut sim, &stmts);
        assert_eq!(result, TestResult::Pass);
        assert_eq!(sim.get("cnt").unwrap(), Value::new(5, 32, false));
    }
}

#[test]
fn testbench_finish_stops_execution() {
    let code = r#"
    module Top (
        clk: input clock,
        rst: input reset,
        cnt: output logic<32>,
    ) {
        always_ff {
            if_reset { cnt = 0; }
            else { cnt += 1; }
        }
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        let stmts = vec![
            TestbenchStatement::ResetAssert {
                reset: rst.clone(),
                clock: clk.clone(),
                duration: 1,
                high_time: 1,
                low_time: 1,
            },
            TestbenchStatement::For {
                count: 5,
                body: vec![TestbenchStatement::ClockNext {
                    clock: clk.clone(),
                    count: None,
                    high_time: 1,
                    low_time: 1,
                }],
                loop_var: None,
            },
            TestbenchStatement::Finish,
            // This should not execute
            TestbenchStatement::For {
                count: 100,
                body: vec![TestbenchStatement::ClockNext {
                    clock: clk.clone(),
                    count: None,
                    high_time: 1,
                    low_time: 1,
                }],
                loop_var: None,
            },
        ];

        let result = run_testbench(&mut sim, &stmts);
        assert_eq!(result, TestResult::Pass);
        // Counter should be 5, not 105
        assert_eq!(sim.get("cnt").unwrap(), Value::new(5, 32, false));
    }
}

#[test]
fn tb_clock_reset_analyze() {
    // Verify that $tb::clock_gen/$tb::reset_gen and method calls pass analyzer in test module
    symbol_table::clear();
    let code = r#"
    #[test(test_counter)]
    module test_counter {
        inst clk: $tb::clock_gen;
        inst rst: $tb::reset_gen(clk);

        initial {
            rst.assert();
            clk.next(10);
            clk.next();
            $finish();
        }
    }
    "#;
    let metadata = Metadata::create_default("prj").unwrap();
    let parser = Parser::parse(&code, &"").unwrap();
    let analyzer = Analyzer::new(&metadata);
    let mut context = Context::default();

    let mut errors = vec![];
    let mut ir = air::Ir::default();
    errors.append(&mut analyzer.analyze_pass1(&"prj", &parser.veryl));
    errors.append(&mut Analyzer::analyze_post_pass1());
    errors.append(&mut analyzer.analyze_pass2(&"prj", &parser.veryl, &mut context, Some(&mut ir)));
    errors.append(&mut Analyzer::analyze_post_pass2());

    let errors: Vec<_> = errors
        .drain(0..)
        .filter(|x| !matches!(x, AnalyzerError::InvalidLogicalOperand { .. }))
        .collect();
    assert!(errors.is_empty(), "Expected no analyzer errors");
}

#[test]
fn tb_integration_counter() {
    let code = r#"
    module Counter (
        clk: input clock,
        rst: input reset,
        cnt: output logic<32>,
    ) {
        always_ff {
            if_reset { cnt = 0; }
            else { cnt += 1; }
        }
    }

    #[test(test_counter)]
    module test_counter {
        inst clk: $tb::clock_gen;
        inst rst: $tb::reset_gen(clk);

        var cnt: logic<32>;

        inst dut: Counter (
            clk: clk,
            rst: rst,
            cnt: cnt,
        );

        initial {
            rst.assert();
            clk.next(10);
            $finish();
        }
    }
    "#;

    for config in Config::all() {
        let ir = analyze_top(code, &config, "test_counter");
        let ir = match ir {
            Ok(ir) => ir,
            Err(_) => continue,
        };

        let mut sim = Simulator::new(ir, None);

        let event_map = build_event_map(&sim.ir.event_statements, &sim.ir.module_variables);
        let clock_periods = build_clock_periods(&sim.ir.event_statements);

        // Get initial block statements and convert to testbench
        let initial_stmts = sim.ir.event_statements.get(&Event::Initial);
        if let Some(stmts) = initial_stmts {
            let tb_stmts = convert_initial_to_testbench(stmts, &event_map, &clock_periods, 3);
            let result = run_testbench(&mut sim, &tb_stmts);
            assert_eq!(result, TestResult::Pass);
            let cnt = sim
                .get_var("dut.cnt")
                .or_else(|| sim.get_var("cnt"))
                .expect("cnt variable not found");
            assert_eq!(cnt, Value::new(10, 32, false));
        } else {
            panic!("No initial block found");
        }
    }
}

/// Reproduce veryl-test read-only cache issue via testbench path
#[test]
fn tb_readonly_cache_fill() {
    // Dummy modules before the actual test to pollute symbol_table
    // (simulates std library files being processed first)
    let prefix = r#"
    module DummyA (clk: input clock, rst: input reset, a: input logic<32>, b: output logic<32>) {
        var x: logic<32>;
        always_ff { if_reset { x = 0; } else { x = a; } }
        assign b = x;
    }
    module DummyB (clk: input clock, rst: input reset, c: input logic<16>, d: output logic<16>) {
        var y: logic<16>;
        always_ff { if_reset { y = 0; } else { y = c + 16'd1; } }
        assign d = y;
    }
    "#;
    let code_main = r#"
    module Harness (
        clk: input clock, rst: input reset,
        o_r1: output logic<64>, o_r2: output logic<64>, o_stall: output logic,
    ) {
        var addr: logic<64>; var ren: logic; var rdata: logic<64>;
        var stall: logic; var mem_addr: logic<64>; var mem_ren: logic;
        var mem_rdata: logic<64>;
        inst dut: Cache (clk: clk, rst: rst, i_addr: addr, i_ren: ren,
            o_rdata: rdata, o_stall: stall,
            o_mem_addr: mem_addr, o_mem_ren: mem_ren, i_mem_rdata: mem_rdata);
        var mem: logic<64> [256];
        assign mem_rdata = mem[mem_addr[10:3]];
        var tc: logic<8>; var r1_val: logic<64>; var r2_val: logic<64>;
        always_ff (clk, rst) {
            if_reset { tc = 0; ren = 0; addr = 0; r1_val = 0; r2_val = 0;
                for i: i32 in 0..256 {
                    if i == 0 { mem[i] = 64'h0000_0000_0000_AAAA; }
                    else if i == 1 { mem[i] = 64'h0000_0000_0000_BBBB; }
                    else { mem[i] = 0; }
                }
            } else {
                ren = 0;
                if stall { ren = 1; }
                if !stall { tc = tc + 8'd1; }
                case tc {
                    8'd1: { addr = 64'h0; ren = 1; }
                    8'd3: { addr = 64'h0; ren = 1; }
                    8'd4: { r1_val = rdata; }
                    8'd6: { addr = 64'h8; ren = 1; }
                    8'd7: { r2_val = rdata; }
                    default: {}
                }
            }
        }
        assign o_r1 = r1_val;
        assign o_r2 = r2_val;
        assign o_stall = stall;
    }
    #[test(test_cache)]
    module test_cache {
        inst clk: $tb::clock_gen;
        inst rst: $tb::reset_gen(clk);
        var r1: logic<64>; var r2: logic<64>;
        var stall: logic;
        inst h: Harness (clk: clk, rst: rst, o_r1: r1, o_r2: r2, o_stall: stall);
        initial {
            rst.assert();
            clk.next(20);
            $assert(r2 == 64'h0000_0000_0000_BBBB, "r2 wrong");
            $finish();
        }
    }
    "#;

    // Test with multi-file processing: Cache in one file, test in another
    // (like heliodor's src/cache/icache.veryl + tb/test_icache.veryl)
    let cache_file = r#"
    pub module Cache (
        clk: input clock, rst: input reset,
        i_addr: input logic<64>, i_ren: input logic,
        o_rdata: output logic<64>, o_stall: output logic,
        o_mem_addr: output logic<64>, o_mem_ren: output logic,
        i_mem_rdata: input logic<64>,
    ) {
        let tag: logic<54> = i_addr[63:10];
        let index: logic<4> = i_addr[9:6];
        let offset: logic<3> = i_addr[5:3];
        let data_idx: logic<7> = {index, offset};
        var valid: logic<16>;
        var tags: logic<54> [16];
        var data: logic<64> [128];
        let cache_hit: logic = valid[index] && tags[index] == tag;
        var state: logic<2>;
        var fill_count: logic<3>;
        var fill_index: logic<4>;
        var fill_tag: logic<54>;
        let fill_data_idx: logic<7> = {fill_index, fill_count};
        let miss: logic = i_ren && !cache_hit && state == 2'd0;
        let filling: logic = state == 2'd1;
        always_ff (clk, rst) {
            if_reset {
                state = 0; fill_count = 0; fill_index = 0; fill_tag = 0; valid = 0;
                for i: i32 in 0..16 { tags[i] = 0; }
                for i: i32 in 0..128 { data[i] = 0; }
            } else {
                case state {
                    2'd0: { if miss { fill_index = index; fill_tag = tag; fill_count = 0; state = 2'd1; } }
                    2'd1: {
                        data[fill_data_idx] = i_mem_rdata;
                        if fill_count == 3'd7 { tags[fill_index] = fill_tag; valid[fill_index] = 1; state = 2'd2; }
                        else { fill_count = fill_count + 3'd1; }
                    }
                    2'd2: { state = 0; }
                    default: { state = 0; }
                }
            }
        }
        assign o_rdata = if cache_hit ? data[data_idx] : i_mem_rdata;
        assign o_stall = filling || miss;
        assign o_mem_addr = if filling ? {fill_tag, fill_index, fill_count, 3'b000}
                          : if i_ren && !cache_hit ? {tag, index, 3'd0, 3'b000}
                          : '0;
        assign o_mem_ren = if filling ? 1'b1 : if i_ren && !cache_hit ? 1'b1 : 1'b0;
    }
    "#;
    let _ = prefix; // suppress unused warning
    for config in Config::all() {
        dbg!(&config);
        // Generate many dummy modules to shift symbol IDs (simulate std library)
        let mut std_code = String::new();
        for j in 0..50 {
            std_code.push_str(&format!(
                "pub module dummy_{j} (clk: input clock, rst: input reset, a: input logic<32>, b: output logic<32>) {{
                    var x: logic<32>;
                    always_ff (clk, rst) {{ if_reset {{ x = 0; }} else {{ x = a + 32'd{j}; }} }}
                    assign b = x;
                }}\n"));
        }
        let ir = analyze_multi_file_prj(
            &[&std_code, cache_file, code_main],
            &config,
            "test_cache",
            &["std", "prj", "prj"],
        );
        let ir = match ir {
            Ok(ir) => ir,
            Err(e) => {
                eprintln!("skip: {:?}", e);
                continue;
            }
        };

        let mut sim = Simulator::new(ir, None);
        let event_map = build_event_map(&sim.ir.event_statements, &sim.ir.module_variables);
        let clock_periods = build_clock_periods(&sim.ir.event_statements);
        let initial_stmts = sim.ir.event_statements.get(&Event::Initial).unwrap();
        let tb_stmts = convert_initial_to_testbench(initial_stmts, &event_map, &clock_periods, 3);
        let result = run_testbench(&mut sim, &tb_stmts);
        assert_eq!(
            result,
            TestResult::Pass,
            "JIT={} 4state={}: testbench failed",
            config.use_jit,
            config.use_4state
        );
    }
}

#[test]
fn tb_function_inline() {
    let code = r#"
    module Counter (
        clk: input clock,
        rst: input reset,
        cnt: output logic<32>,
    ) {
        always_ff {
            if_reset { cnt = 0; }
            else { cnt += 1; }
        }
    }

    #[test(test_inline)]
    module test_inline {
        inst clk: $tb::clock_gen;
        inst rst: $tb::reset_gen(clk);

        var cnt: logic<32>;

        inst dut: Counter (
            clk: clk,
            rst: rst,
            cnt: cnt,
        );

        function step_n(n: input logic<32>) {
            clk.next(n);
        }

        initial {
            rst.assert();
            step_n(5);
            step_n(5);
            $finish();
        }
    }
    "#;

    for config in Config::all() {
        let ir = analyze_top(code, &config, "test_inline");
        let ir = match ir {
            Ok(ir) => ir,
            Err(_) => continue,
        };

        let mut sim = Simulator::new(ir, None);

        let event_map = build_event_map(&sim.ir.event_statements, &sim.ir.module_variables);
        let clock_periods = build_clock_periods(&sim.ir.event_statements);

        let initial_stmts = sim.ir.event_statements.get(&Event::Initial);
        if let Some(stmts) = initial_stmts {
            let tb_stmts = convert_initial_to_testbench(stmts, &event_map, &clock_periods, 3);
            let result = run_testbench(&mut sim, &tb_stmts);
            assert_eq!(result, TestResult::Pass);
            let cnt = sim
                .get_var("dut.cnt")
                .or_else(|| sim.get_var("cnt"))
                .expect("cnt variable not found");
            // reset(3) + step_n(5) + step_n(5) = 10
            assert_eq!(cnt, Value::new(10, 32, false));
        } else {
            panic!("No initial block found");
        }
    }
}

#[test]
fn tb_initial_assign_comb() {
    // Test that comb variable assignment in initial block propagates
    // through a purely combinational (assign) DUT.
    let code = r#"
    module Doubler (
        clk: input clock,
        rst: input reset,
        val: input logic<8>,
        doubled: output logic<8>,
    ) {
        assign doubled = val + val;
    }

    #[test(test_comb_assign)]
    module test_comb_assign {
        inst clk: $tb::clock_gen;
        inst rst: $tb::reset_gen(clk);

        var input_val: logic<8>;
        var output_doubled: logic<8>;

        inst dut: Doubler (
            clk: clk,
            rst: rst,
            val: input_val,
            doubled: output_doubled,
        );

        initial {
            rst.assert();
            input_val = 21;
            clk.next(1);
            $assert(output_doubled == 42, "comb assign failed");
            $finish();
        }
    }
    "#;

    for config in Config::all() {
        let ir = analyze_top(code, &config, "test_comb_assign");
        let ir = match ir {
            Ok(ir) => ir,
            Err(_) => continue,
        };
        let module_name = ir.name.to_string();
        let result = run_native_testbench(ir, None, module_name);
        assert_eq!(
            result.unwrap(),
            TestResult::Pass,
            "tb_initial_assign_comb failed (jit={}, 4state={})",
            config.use_jit,
            config.use_4state,
        );
    }
}

#[test]
fn tb_initial_assign_ff() {
    // Test FF variable assignment in initial + clock step
    let code = r#"
    module Accumulator (
        clk: input clock,
        rst: input reset,
        val: input logic<8>,
        sum: output logic<8>,
    ) {
        always_ff {
            if_reset { sum = 0; }
            else { sum = sum + val; }
        }
    }

    #[test(test_ff_assign)]
    module test_ff_assign {
        inst clk: $tb::clock_gen;
        inst rst: $tb::reset_gen(clk);

        var input_val: logic<8>;
        var output_sum: logic<8>;

        inst dut: Accumulator (
            clk: clk,
            rst: rst,
            val: input_val,
            sum: output_sum,
        );

        initial {
            rst.assert();
            input_val = 10;
            clk.next(3);
            $assert(output_sum == 30, "ff assign failed");
            $finish();
        }
    }
    "#;

    for config in Config::all() {
        let ir = analyze_top(code, &config, "test_ff_assign");
        let ir = match ir {
            Ok(ir) => ir,
            Err(_) => continue,
        };
        let module_name = ir.name.to_string();
        let result = run_native_testbench(ir, None, module_name);
        assert_eq!(
            result.unwrap(),
            TestResult::Pass,
            "tb_initial_assign_ff failed (jit={}, 4state={})",
            config.use_jit,
            config.use_4state,
        );
    }
}

#[test]
fn tb_initial_assign_multiple() {
    // Test multiple variable assignments in initial block.
    // Purely combinational DUT verifies both inputs propagate correctly.
    let code = r#"
    module Adder (
        clk: input clock,
        rst: input reset,
        a: input logic<8>,
        b: input logic<8>,
        sum: output logic<8>,
    ) {
        assign sum = a + b;
    }

    #[test(test_multi_assign)]
    module test_multi_assign {
        inst clk: $tb::clock_gen;
        inst rst: $tb::reset_gen(clk);

        var in_a: logic<8>;
        var in_b: logic<8>;
        var out_sum: logic<8>;

        inst dut: Adder (
            clk: clk,
            rst: rst,
            a: in_a,
            b: in_b,
            sum: out_sum,
        );

        initial {
            rst.assert();
            in_a = 20;
            in_b = 22;
            clk.next(1);
            $assert(out_sum == 42, "multi assign failed");
            $finish();
        }
    }
    "#;

    for config in Config::all() {
        let ir = analyze_top(code, &config, "test_multi_assign");
        let ir = match ir {
            Ok(ir) => ir,
            Err(_) => continue,
        };
        let module_name = ir.name.to_string();
        let result = run_native_testbench(ir, None, module_name);
        assert_eq!(
            result.unwrap(),
            TestResult::Pass,
            "tb_initial_assign_multiple failed (jit={}, 4state={})",
            config.use_jit,
            config.use_4state,
        );
    }
}

#[test]
fn testbench_vcd_clock_reset_waveform() {
    let code = r#"
    module Top (
        clk: input clock,
        rst: input reset,
        cnt: output logic<32>,
    ) {
        always_ff {
            if_reset { cnt = 0; }
            else { cnt += 1; }
        }
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);

        use crate::wave_dumper::{SharedVec, WaveDumper};
        let dump_buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let dumper = WaveDumper::new_vcd(Box::new(SharedVec(dump_buf.clone())));
        let mut sim = Simulator::new(ir, Some(dumper));

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        // Reset for 2 cycles, then clock 3 cycles
        let stmts = vec![
            TestbenchStatement::ResetAssert {
                reset: rst.clone(),
                clock: clk.clone(),
                duration: 2,
                high_time: 1,
                low_time: 1,
            },
            TestbenchStatement::For {
                count: 3,
                body: vec![TestbenchStatement::ClockNext {
                    clock: clk.clone(),
                    count: None,
                    high_time: 1,
                    low_time: 1,
                }],
                loop_var: None,
            },
        ];

        let result = run_testbench(&mut sim, &stmts);
        assert_eq!(result, TestResult::Pass);
        assert_eq!(sim.get("cnt").unwrap(), Value::new(3, 32, false));

        // Parse VCD with the vcd crate
        drop(sim);
        let dump = std::sync::Arc::try_unwrap(dump_buf)
            .unwrap()
            .into_inner()
            .unwrap();
        let mut parser = vcd::Parser::new(dump.as_slice());
        let header = parser.parse_header().unwrap();

        let clk_var = header
            .find_var(&["Top", "clk"])
            .expect("clk not found in VCD");
        let rst_var = header
            .find_var(&["Top", "rst"])
            .expect("rst not found in VCD");
        let clk_code = clk_var.code;
        let rst_code = rst_var.code;

        // Collect per-signal value changes from the VCD body
        let mut clk_values: Vec<(u64, bool)> = Vec::new();
        let mut rst_values: Vec<(u64, bool)> = Vec::new();
        let mut current_time: u64 = 0;
        for cmd in parser {
            match cmd.unwrap() {
                vcd::Command::Timestamp(t) => current_time = t,
                vcd::Command::ChangeVector(code, vec) => {
                    let bit = vec.get(0) == Some(vcd::Value::V1);
                    if code == clk_code {
                        clk_values.push((current_time, bit));
                    } else if code == rst_code {
                        rst_values.push((current_time, bit));
                    }
                }
                vcd::Command::ChangeScalar(code, val) => {
                    let bit = val == vcd::Value::V1;
                    if code == clk_code {
                        clk_values.push((current_time, bit));
                    } else if code == rst_code {
                        rst_values.push((current_time, bit));
                    }
                }
                _ => {}
            }
        }

        // Clock should toggle: 1 (posedge) -> 0 (negedge) in every cycle
        // ResetAssert: 2 cycles = 4 timestamps (posedge+negedge each)
        // ClockNext: 3 cycles = 6 timestamps
        // Total: 10 timestamps
        assert!(
            clk_values.len() >= 10,
            "expected >= 10 clock transitions, got {} (jit={}, 4state={})",
            clk_values.len(),
            config.use_jit,
            config.use_4state,
        );

        // Verify clock toggles: even indices = 1 (posedge), odd indices = 0 (negedge)
        for (i, &(_, val)) in clk_values.iter().enumerate() {
            let expected = i % 2 == 0;
            assert_eq!(
                val, expected,
                "clock value at index {i} should be {expected}, got {val} (jit={}, 4state={})",
                config.use_jit, config.use_4state,
            );
        }

        // Reset should be asserted during reset phase and deasserted after
        assert!(
            !rst_values.is_empty(),
            "expected reset signal changes in VCD (jit={}, 4state={})",
            config.use_jit,
            config.use_4state,
        );
        assert!(
            rst_values[0].1,
            "reset should be asserted initially (jit={}, 4state={})",
            config.use_jit, config.use_4state,
        );
        assert!(
            !rst_values.last().unwrap().1,
            "reset should be deasserted after reset phase (jit={}, 4state={})",
            config.use_jit,
            config.use_4state,
        );
    }
}

#[test]
fn testbench_fst_clock_reset_waveform() {
    let code = r#"
    module Top (
        clk: input clock,
        rst: input reset,
        cnt: output logic<32>,
    ) {
        always_ff {
            if_reset { cnt = 0; }
            else { cnt += 1; }
        }
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);

        let fst_path = format!(
            "{}/fst_test_{}_{}.fst",
            std::env::temp_dir().display(),
            config.use_jit,
            config.use_4state,
        );

        use crate::wave_dumper::WaveDumper;
        let dumper = WaveDumper::new_fst(&fst_path);
        let mut sim = Simulator::new(ir, Some(dumper));

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        let stmts = vec![
            TestbenchStatement::ResetAssert {
                reset: rst.clone(),
                clock: clk.clone(),
                duration: 2,
                high_time: 1,
                low_time: 1,
            },
            TestbenchStatement::For {
                count: 3,
                body: vec![TestbenchStatement::ClockNext {
                    clock: clk.clone(),
                    count: None,
                    high_time: 1,
                    low_time: 1,
                }],
                loop_var: None,
            },
        ];

        let result = run_testbench(&mut sim, &stmts);
        assert_eq!(result, TestResult::Pass);
        assert_eq!(sim.get("cnt").unwrap(), Value::new(3, 32, false));
        drop(sim);

        // Read FST back with wellen
        let mut wave = wellen::simple::read(&fst_path).expect("failed to read FST");
        let hier = wave.hierarchy();

        // Find clk and rst variables
        let clk_var = hier
            .iter_vars()
            .find(|v| v.name(hier) == "clk")
            .expect("clk not found in FST");
        let rst_var = hier
            .iter_vars()
            .find(|v| v.name(hier) == "rst")
            .expect("rst not found in FST");
        let clk_ref = clk_var.signal_ref();
        let rst_ref = rst_var.signal_ref();

        wave.load_signals(&[clk_ref, rst_ref]);

        let time_table = wave.time_table();
        let clk_signal = wave.get_signal(clk_ref).unwrap();
        let rst_signal = wave.get_signal(rst_ref).unwrap();

        // Collect clock values at each time step
        let mut clk_values: Vec<(u64, u8)> = Vec::new();
        for (i, &t) in time_table.iter().enumerate() {
            if let Some(offset) = clk_signal.get_offset(i as u32) {
                let val = clk_signal.get_value_at(&offset, 0);
                if let wellen::SignalValue::Binary(bits, _) = val {
                    clk_values.push((t, bits[0]));
                }
            }
        }

        // Clock should toggle across timestamps
        assert!(
            clk_values.len() >= 10,
            "expected >= 10 clock transitions in FST, got {} (jit={}, 4state={})",
            clk_values.len(),
            config.use_jit,
            config.use_4state,
        );

        for (i, &(_, val)) in clk_values.iter().enumerate() {
            let expected: u8 = if i % 2 == 0 { 1 } else { 0 };
            assert_eq!(
                val, expected,
                "FST clock at index {i} should be {expected}, got {val} (jit={}, 4state={})",
                config.use_jit, config.use_4state,
            );
        }

        // Collect reset values
        let mut rst_values: Vec<(u64, u8)> = Vec::new();
        for (i, &t) in time_table.iter().enumerate() {
            if let Some(offset) = rst_signal.get_offset(i as u32) {
                let val = rst_signal.get_value_at(&offset, 0);
                if let wellen::SignalValue::Binary(bits, _) = val {
                    rst_values.push((t, bits[0]));
                }
            }
        }

        assert!(
            !rst_values.is_empty(),
            "expected reset signal changes in FST (jit={}, 4state={})",
            config.use_jit,
            config.use_4state,
        );
        assert_eq!(
            rst_values[0].1, 1,
            "reset should be asserted initially in FST (jit={}, 4state={})",
            config.use_jit, config.use_4state,
        );
        assert_eq!(
            rst_values.last().unwrap().1,
            0,
            "reset should be deasserted after reset phase in FST (jit={}, 4state={})",
            config.use_jit,
            config.use_4state,
        );

        std::fs::remove_file(&fst_path).ok();
    }
}

#[test]
fn testbench_array_input_port() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        cnt: output logic<32>,
    ) {
        var arr: logic<32> [2];
        always_ff {
            if_reset {
                arr[0] = 0;
                arr[1] = 0;
            } else {
                arr[0] += 1;
                arr[1] += 2;
            }
        }

        inst u: ArraySub (
            clk,
            rst,
            i_x: arr,
            cnt,
        );
    }

    module ArraySub (
        clk: input  clock,
        rst: input  reset,
        i_x: input  logic<32> [2],
        cnt: output logic<32>,
    ) {
        assign cnt = i_x[0] + i_x[1];
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        let stmts = vec![
            TestbenchStatement::ResetAssert {
                reset: rst.clone(),
                clock: clk.clone(),
                duration: 3,
                high_time: 1,
                low_time: 1,
            },
            TestbenchStatement::For {
                count: 5,
                body: vec![TestbenchStatement::ClockNext {
                    clock: clk.clone(),
                    count: None,
                    high_time: 1,
                    low_time: 1,
                }],
                loop_var: None,
            },
            TestbenchStatement::Finish,
        ];

        let result = run_testbench(&mut sim, &stmts);
        assert_eq!(result, TestResult::Pass);
        // After 5 cycles: arr[0]=5, arr[1]=10, cnt=15
        assert_eq!(sim.get("cnt").unwrap(), Value::new(15, 32, false));
    }
}

#[test]
fn testbench_vcd_comb_only_clock_reset() {
    let code = r#"
    module CombOnly (
        clk: input clock,
        rst: input reset,
        a: input logic<8>,
        b: output logic<8>,
    ) {
        assign b = a + 8'd1;
    }

    #[test(test_comb)]
    module test_comb {
        inst clk: $tb::clock_gen;
        inst rst: $tb::reset_gen(clk);

        var b: logic<8>;

        inst dut: CombOnly (
            clk: clk,
            rst: rst,
            a: 8'd42,
            b: b,
        );

        initial {
            rst.assert();
            clk.next(3);
            $finish();
        }
    }
    "#;

    for config in Config::all() {
        let ir = analyze_top(code, &config, "test_comb");
        let ir = match ir {
            Ok(ir) => ir,
            Err(_) => continue,
        };

        use crate::wave_dumper::{SharedVec, WaveDumper};
        let dump_buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let dumper = WaveDumper::new_vcd(Box::new(SharedVec(dump_buf.clone())));
        let mut sim = Simulator::new(ir, Some(dumper));

        let event_map = build_event_map(&sim.ir.event_statements, &sim.ir.module_variables);
        let clock_periods = build_clock_periods(&sim.ir.event_statements);

        let initial_stmts = sim.ir.event_statements.get(&Event::Initial);
        assert!(initial_stmts.is_some(), "No initial block found");
        let tb_stmts =
            convert_initial_to_testbench(initial_stmts.unwrap(), &event_map, &clock_periods, 3);
        let result = run_testbench(&mut sim, &tb_stmts);
        assert_eq!(result, TestResult::Pass);
        drop(sim);

        let dump = std::sync::Arc::try_unwrap(dump_buf)
            .unwrap()
            .into_inner()
            .unwrap();
        let mut parser = vcd::Parser::new(dump.as_slice());
        let header = parser.parse_header().unwrap();

        let clk_var = header.find_var(&["test_comb", "clk"]);
        let rst_var = header.find_var(&["test_comb", "rst"]);

        assert!(
            clk_var.is_some(),
            "clk not found in VCD (jit={}, 4state={})",
            config.use_jit,
            config.use_4state,
        );
        assert!(
            rst_var.is_some(),
            "rst not found in VCD (jit={}, 4state={})",
            config.use_jit,
            config.use_4state,
        );

        let clk_code = clk_var.unwrap().code;
        let rst_code = rst_var.unwrap().code;

        let mut clk_values: Vec<(u64, bool)> = Vec::new();
        let mut rst_values: Vec<(u64, bool)> = Vec::new();
        let mut current_time: u64 = 0;
        for cmd in parser {
            match cmd.unwrap() {
                vcd::Command::Timestamp(t) => current_time = t,
                vcd::Command::ChangeVector(code, vec) => {
                    let bit = vec.get(0) == Some(vcd::Value::V1);
                    if code == clk_code {
                        clk_values.push((current_time, bit));
                    } else if code == rst_code {
                        rst_values.push((current_time, bit));
                    }
                }
                vcd::Command::ChangeScalar(code, val) => {
                    let bit = val == vcd::Value::V1;
                    if code == clk_code {
                        clk_values.push((current_time, bit));
                    } else if code == rst_code {
                        rst_values.push((current_time, bit));
                    }
                }
                _ => {}
            }
        }

        // rst.assert() = 3 cycles (default duration) + clk.next(3) = 3 cycles
        // Each cycle: posedge(1) + negedge(0) = 2 transitions
        // Total clock transitions >= 12
        assert!(
            clk_values.len() >= 12,
            "expected >= 12 clock transitions for comb-only DUT, got {} (jit={}, 4state={})",
            clk_values.len(),
            config.use_jit,
            config.use_4state,
        );

        for (i, &(_, val)) in clk_values.iter().enumerate() {
            let expected = i % 2 == 0;
            assert_eq!(
                val, expected,
                "clock value at index {i} should be {expected}, got {val} (jit={}, 4state={})",
                config.use_jit, config.use_4state,
            );
        }

        assert!(
            !rst_values.is_empty(),
            "expected reset signal changes in VCD for comb-only DUT (jit={}, 4state={})",
            config.use_jit,
            config.use_4state,
        );
        assert!(
            rst_values[0].1,
            "reset should be asserted initially (jit={}, 4state={})",
            config.use_jit, config.use_4state,
        );
        assert!(
            !rst_values.last().unwrap().1,
            "reset should be deasserted after reset phase (jit={}, 4state={})",
            config.use_jit,
            config.use_4state,
        );
    }
}

#[test]
fn tb_dual_clock() {
    let code = r#"
    module DualClock (
        clk_a: input  'a clock    ,
        rst_a: input  'a reset    ,
        clk_b: input  'b clock    ,
        rst_b: input  'b reset    ,
        cnt_a: output 'a logic<32>,
        cnt_b: output 'b logic<32>,
    ) {
        always_ff (clk_a, rst_a) {
            if_reset { cnt_a = 0; }
            else     { cnt_a += 1; }
        }
        always_ff (clk_b, rst_b) {
            if_reset { cnt_b = 0; }
            else     { cnt_b += 1; }
        }
    }

    #[test(test_dual_clock)]
    module test_dual_clock {
        inst clk_a: $tb::clock_gen;
        inst rst_a: $tb::reset_gen(clk: clk_a);
        inst clk_b: $tb::clock_gen;
        inst rst_b: $tb::reset_gen(clk: clk_b);

        var cnt_a: logic<32>;
        var cnt_b: logic<32>;

        inst dut: DualClock (
            clk_a, rst_a, clk_b, rst_b, cnt_a, cnt_b,
        );

        initial {
            rst_a.assert();
            rst_b.assert();
            clk_a.next(10);
            $assert(cnt_a == 32'd10);
            $assert(cnt_b == 32'd0);
            clk_b.next(5);
            $assert(cnt_a == 32'd10);
            $assert(cnt_b == 32'd5);
            $finish();
        }
    }
    "#;

    for config in Config::all() {
        let ir = analyze_top(code, &config, "test_dual_clock");
        let ir = match ir {
            Ok(ir) => ir,
            Err(_) => continue,
        };
        let module_name = ir.name.to_string();
        let result = run_native_testbench(ir, None, module_name);
        assert_eq!(
            result.unwrap(),
            TestResult::Pass,
            "tb_dual_clock failed (jit={}, 4state={})",
            config.use_jit,
            config.use_4state,
        );
    }
}

#[test]
fn tb_const_function_with_if() {
    // A const parameter computed by a function containing if/else
    // must be correctly evaluated so that port widths are resolved.
    let code = r#"
    package pkg {
        function calc_width(kind: input u32) -> u32 {
            if kind == 0 {
                return 4;
            } else {
                return 8;
            }
        }
    }

    module Dut #(
        param KIND: u32 = 0,
        const W: u32 = pkg::calc_width(KIND),
    ) (
        clk  : input  clock   ,
        rst  : input  reset   ,
        i_val: input  logic<W>,
        o_val: output logic<W>,
    ) {
        assign o_val = i_val;
    }

    #[test(test_const_if)]
    module test_const_if {
        inst clk: $tb::clock_gen;
        inst rst: $tb::reset_gen(clk);

        var i_val: logic<4>;
        var o_val: logic<4>;

        inst dut: Dut #(KIND: 0) (
            clk, rst, i_val, o_val,
        );

        initial {
            rst.assert();
            i_val = 4'd9;
            clk.next(1);
            $assert(o_val == 4'd9, "const if eval failed");
            $finish();
        }
    }
    "#;

    for config in Config::all() {
        let ir = analyze_top(code, &config, "test_const_if");
        let ir = match ir {
            Ok(ir) => ir,
            Err(_) => continue,
        };
        let module_name = ir.name.to_string();
        let result = run_native_testbench(ir, None, module_name);
        assert_eq!(
            result.unwrap(),
            TestResult::Pass,
            "tb_const_function_with_if failed (jit={}, 4state={})",
            config.use_jit,
            config.use_4state,
        );
    }
}

#[test]
fn tb_assert_arith_in_initial() {
    let code = r#"
    #[test(test_foo)]
    module test_foo {
        var a: u32;
        var b: u32;
        initial {
            a = 1;
            b = a + 1;
            $assert(b == (a + 1));
            $finish();
        }
    }
    "#;

    for config in Config::all() {
        let ir = analyze_top(code, &config, "test_foo");
        let ir = match ir {
            Ok(ir) => ir,
            Err(_) => continue,
        };
        let module_name = ir.name.to_string();
        let result = run_native_testbench(ir, None, module_name);
        assert_eq!(
            result.unwrap(),
            TestResult::Pass,
            "tb_assert_arith_in_initial failed (jit={}, 4state={})",
            config.use_jit,
            config.use_4state,
        );
    }
}

#[test]
fn tb_2d_packed_select() {
    // Selecting a row from a 2D packed type.
    // `h: logic<2, 3>` is 6 bits.  `h[0]` must select the first
    // 3-bit row (bits [2:0]), not just bit 0.
    let code = r#"
    module Test2dPacked (
        clk  : input  clock   ,
        rst  : input  reset   ,
        o_val: output logic<3>,
    ) {
        var h: logic<2, 3>;
        assign h[0][0] = 1'b1;
        assign h[0][1] = 1'b0;
        assign h[0][2] = 1'b1;
        assign h[1][0] = 1'b0;
        assign h[1][1] = 1'b1;
        assign h[1][2] = 1'b1;

        assign o_val = h[0];
    }

    #[test(test_2d)]
    module test_2d {
        inst clk: $tb::clock_gen;
        inst rst: $tb::reset_gen(clk);
        var o_val: logic<3>;
        inst dut: Test2dPacked(clk, rst, o_val);
        initial {
            rst.assert();
            clk.next(1);
            $assert(o_val == 3'b101, "h[0] should be 101");
            $finish();
        }
    }
    "#;

    for config in Config::all() {
        let ir = analyze_top(code, &config, "test_2d");
        let ir = match ir {
            Ok(ir) => ir,
            Err(_) => continue,
        };
        let module_name = ir.name.to_string();
        let result = run_native_testbench(ir, None, module_name);
        assert_eq!(
            result.unwrap(),
            TestResult::Pass,
            "tb_2d_packed_select failed (jit={}, 4state={})",
            config.use_jit,
            config.use_4state,
        );
    }
}

#[test]
fn tb_multi_inst_distinct_params() {
    // Regression for veryl-lang/veryl#2557.
    let code = r#"
    module Foo #(
        param length: u32 = 32,
    ) (
        i: input  logic<length>,
        o: output logic<length>,
    ) {
        assign o = i;
    }

    #[test(test_multi_inst)]
    module test_multi_inst {
        inst clk: $tb::clock_gen;
        inst rst: $tb::reset_gen(clk);

        var i_32: logic<32>;
        var o_32: logic<32>;

        var i_5: logic<5>;
        var o_5: logic<5>;

        inst foo_5: Foo #(
            length: 5,
        ) (
            i: i_5,
            o: o_5,
        );

        inst foo_32: Foo #(
            length: 32,
        ) (
            i: i_32,
            o: o_32,
        );

        initial {
            rst.assert();
            i_32 = 'hFFFFFFFF;
            i_5 = 'b11111;
            clk.next(1);
            $assert(o_5 == 'b11111, "Incorrect for 5-bit wide module");
            $assert(o_32 == 'hFFFFFFFF, "Incorrect for 32-bit wide module");
            $finish();
        }
    }
    "#;

    for config in Config::all() {
        let ir = analyze_top(code, &config, "test_multi_inst");
        let ir = match ir {
            Ok(ir) => ir,
            Err(_) => continue,
        };
        let module_name = ir.name.to_string();
        let result = run_native_testbench(ir, None, module_name);
        assert_eq!(
            result.unwrap(),
            TestResult::Pass,
            "tb_multi_inst_distinct_params failed (jit={}, 4state={})",
            config.use_jit,
            config.use_4state,
        );
    }
}
