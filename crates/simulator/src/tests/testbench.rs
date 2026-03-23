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
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        let stmts = vec![
            TestbenchStatement::ResetAssert {
                reset: rst.clone(),
                clock: clk.clone(),
                duration: 3,
            },
            TestbenchStatement::For {
                count: 10,
                body: vec![TestbenchStatement::ClockNext {
                    clock: clk.clone(),
                    count: None,
                }],
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
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        let stmts = vec![
            TestbenchStatement::ResetAssert {
                reset: rst.clone(),
                clock: clk.clone(),
                duration: 5,
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
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        let stmts = vec![
            TestbenchStatement::ResetAssert {
                reset: rst.clone(),
                clock: clk.clone(),
                duration: 1,
            },
            // Step 5 times using For loop (each iteration steps 1 clock)
            TestbenchStatement::For {
                count: 5,
                body: vec![TestbenchStatement::ClockNext {
                    clock: clk.clone(),
                    count: None,
                }],
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
        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        let stmts = vec![
            TestbenchStatement::ResetAssert {
                reset: rst.clone(),
                clock: clk.clone(),
                duration: 1,
            },
            TestbenchStatement::For {
                count: 5,
                body: vec![TestbenchStatement::ClockNext {
                    clock: clk.clone(),
                    count: None,
                }],
            },
            TestbenchStatement::Finish,
            // This should not execute
            TestbenchStatement::For {
                count: 100,
                body: vec![TestbenchStatement::ClockNext {
                    clock: clk.clone(),
                    count: None,
                }],
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
        inst rst: $tb::reset_gen;

        initial {
            rst.assert(clk);
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
        inst rst: $tb::reset_gen;

        var cnt: logic<32>;

        inst dut: Counter (
            clk: clk,
            rst: rst,
            cnt: cnt,
        );

        initial {
            rst.assert(clk);
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

        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let event_map = build_event_map(&sim.ir.event_statements);

        // Get initial block statements and convert to testbench
        let initial_stmts = sim.ir.event_statements.get(&Event::Initial);
        if let Some(stmts) = initial_stmts {
            let tb_stmts = convert_initial_to_testbench(stmts, &event_map, 3);
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
        inst rst: $tb::reset_gen;
        var r1: logic<64>; var r2: logic<64>;
        var stall: logic;
        inst h: Harness (clk: clk, rst: rst, o_r1: r1, o_r2: r2, o_stall: stall);
        initial {
            rst.assert(clk);
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

        let mut sim = Simulator::<std::io::Empty>::new(ir, None);
        let event_map = build_event_map(&sim.ir.event_statements);
        let initial_stmts = sim.ir.event_statements.get(&Event::Initial).unwrap();
        let tb_stmts = convert_initial_to_testbench(initial_stmts, &event_map, 3);
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
        inst rst: $tb::reset_gen;

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
            rst.assert(clk);
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

        let mut sim = Simulator::<std::io::Empty>::new(ir, None);

        let event_map = build_event_map(&sim.ir.event_statements);

        let initial_stmts = sim.ir.event_statements.get(&Event::Initial);
        if let Some(stmts) = initial_stmts {
            let tb_stmts = convert_initial_to_testbench(stmts, &event_map, 3);
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
        inst rst: $tb::reset_gen;

        var input_val: logic<8>;
        var output_doubled: logic<8>;

        inst dut: Doubler (
            clk: clk,
            rst: rst,
            val: input_val,
            doubled: output_doubled,
        );

        initial {
            rst.assert(clk);
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
        let result = run_native_testbench(ir, None);
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
        inst rst: $tb::reset_gen;

        var input_val: logic<8>;
        var output_sum: logic<8>;

        inst dut: Accumulator (
            clk: clk,
            rst: rst,
            val: input_val,
            sum: output_sum,
        );

        initial {
            rst.assert(clk);
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
        let result = run_native_testbench(ir, None);
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
        inst rst: $tb::reset_gen;

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
            rst.assert(clk);
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
        let result = run_native_testbench(ir, None);
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

        let mut dump = Vec::new();
        let mut sim = Simulator::new(ir, Some(&mut dump));

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        // Reset for 2 cycles, then clock 3 cycles
        let stmts = vec![
            TestbenchStatement::ResetAssert {
                reset: rst.clone(),
                clock: clk.clone(),
                duration: 2,
            },
            TestbenchStatement::For {
                count: 3,
                body: vec![TestbenchStatement::ClockNext {
                    clock: clk.clone(),
                    count: None,
                }],
            },
        ];

        let result = run_testbench(&mut sim, &stmts);
        assert_eq!(result, TestResult::Pass);
        assert_eq!(sim.get("cnt").unwrap(), Value::new(3, 32, false));

        // Parse VCD with the vcd crate
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
