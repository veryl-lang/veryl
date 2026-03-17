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
    // Verify that $tb::clock_gen/$tb::reset_gen and method calls pass analyzer
    symbol_table::clear();
    let code = r#"
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
