use super::*;

/// Runs the testbench `top` under every simulator configuration.
#[track_caller]
fn run_pass(code: &str, top: &str) {
    for config in Config::all() {
        let ir = analyze_top(code, &config, top).unwrap();
        let module_name = ir.name.to_string();
        assert_eq!(
            run_native_testbench(ir, None, module_name).unwrap(),
            TestResult::Pass,
            "{top} failed (jit={}, 4state={})",
            config.use_jit,
            config.use_4state,
        );
    }
}

#[test]
fn fork_wait_all_joins_after_the_slowest_branch() {
    let code = r#"
    #[test(test_wait_all)]
    module test_wait_all {
        inst clk: $tb::clock_gen;
        var seq  : logic<8>;
        var a    : logic<8>;
        var b    : logic<8>;
        var after: logic<8>;
        initial {
            seq = 0;
            a = 0;
            b = 0;
            after = 0;
            fork(wait_all) {
                { clk.next(5); seq = seq + 1; b = seq; }
                { clk.next(2); seq = seq + 1; a = seq; }
            }
            // `a` finished first, and the join waited for `b` too.
            $assert(a == 1);
            $assert(b == 2);
            seq = seq + 1;
            after = seq;
            $assert(after == 3);
            $finish();
        }
    }
    "#;
    run_pass(code, "test_wait_all");
}

#[test]
fn fork_wait_any_continues_after_the_first_branch() {
    let code = r#"
    #[test(test_wait_any)]
    module test_wait_any {
        inst clk: $tb::clock_gen;
        var seq: logic<8>;
        var a  : logic<8>;
        var b  : logic<8>;
        initial {
            seq = 0;
            a = 0;
            b = 0;
            fork(wait_any) {
                { clk.next(2); seq = seq + 1; a = seq; }
                { clk.next(5); seq = seq + 1; b = seq; }
            }
            // Released by `a`; `b` is still running.
            $assert(a == 1);
            $assert(b == 0);
            clk.next(2);
            $assert(b == 0);
            // ... and finishes on its own afterwards.
            clk.next(2);
            $assert(b == 2);
            $finish();
        }
    }
    "#;
    run_pass(code, "test_wait_any");
}

#[test]
fn fork_wait_none_does_not_block() {
    let code = r#"
    #[test(test_wait_none)]
    module test_wait_none {
        inst clk: $tb::clock_gen;
        var a: logic;
        var b: logic;
        initial {
            a = 0;
            b = 0;
            fork(wait_none) {
                { clk.next(2); a = 1; }
                { clk.next(4); b = 1; }
            }
            // No time has passed.
            $assert(a == 0);
            $assert(b == 0);
            clk.next(3);
            $assert(a == 1);
            $assert(b == 0);
            clk.next(2);
            $assert(b == 1);
            $finish();
        }
    }
    "#;
    run_pass(code, "test_wait_none");
}

#[test]
fn fork_runs_each_statement_as_its_own_branch() {
    let code = r#"
    #[test(test_stmt_branches)]
    module test_stmt_branches {
        inst clk: $tb::clock_gen;
        var a: logic<8>;
        var b: logic<8>;
        var c: logic<8>;
        initial {
            fork(wait_all) {
                a = 1;
                b = 2;
                { clk.next(1); c = a + b; }
            }
            $assert(a == 1);
            $assert(b == 2);
            $assert(c == 3);
            $finish();
        }
    }
    "#;
    run_pass(code, "test_stmt_branches");
}

#[test]
fn fork_nested_orders_the_branches() {
    let code = r#"
    #[test(test_nested)]
    module test_nested {
        inst clk: $tb::clock_gen;
        var seq: logic<8>;
        var x  : logic<8>;
        var y  : logic<8>;
        var z  : logic<8>;
        var w  : logic<8>;
        initial {
            seq = 0;
            x = 0;
            y = 0;
            z = 0;
            w = 0;
            fork(wait_all) {
                {
                    fork(wait_all) {
                        { clk.next(1); seq = seq + 1; x = seq; }
                        { clk.next(3); seq = seq + 1; y = seq; }
                    }
                    // Only after both inner branches.
                    seq = seq + 1;
                    z = seq;
                }
                { clk.next(2); seq = seq + 1; w = seq; }
            }
            $assert(x == 1);
            $assert(w == 2);
            $assert(y == 3);
            $assert(z == 4);
            $finish();
        }
    }
    "#;
    run_pass(code, "test_nested");
}

#[test]
fn disable_fork_kills_descendants_only() {
    let code = r#"
    #[test(test_disable)]
    module test_disable {
        inst clk: $tb::clock_gen;
        var a : logic;
        var g : logic;
        var b : logic;
        initial {
            a = 0;
            g = 0;
            fork(wait_none) {
                { clk.next(5); a = 1; }
                // The grandchild goes down with its parent.
                {
                    fork(wait_none) {
                        { clk.next(6); g = 1; }
                    }
                }
            }
            clk.next(2);
            disable fork;
            clk.next(10);
            $assert(a == 0);
            $assert(g == 0);
            $assert(b == 1);
            $finish();
        }
        // Another process's forks are not this one's descendants.
        initial {
            b = 0;
            fork(wait_none) {
                { clk.next(4); b = 1; }
            }
        }
    }
    "#;
    run_pass(code, "test_disable");
}

#[test]
fn fork_in_a_loop_spawns_every_iteration() {
    let code = r#"
    #[test(test_loop)]
    module test_loop {
        inst clk: $tb::clock_gen;
        var cnt: logic<8>;
        initial {
            cnt = 0;
            for _i in 0..3 {
                fork(wait_none) {
                    { clk.next(1); cnt = cnt + 1; }
                }
            }
            $assert(cnt == 0);
            clk.next(3);
            $assert(cnt == 3);
            $finish();
        }
    }
    "#;
    run_pass(code, "test_loop");
}

#[test]
fn fork_wait_any_then_wait_all_ignores_the_late_branch() {
    // The slow branch of the first fork finishing while the parent waits in
    // a second fork must not release that join.
    let code = r#"
    #[test(test_two_forks)]
    module test_two_forks {
        inst clk: $tb::clock_gen;
        var slow: logic;
        var done: logic;
        initial {
            slow = 0;
            done = 0;
            fork(wait_any) {
                { clk.next(1); }
                { clk.next(2); slow = 1; }
            }
            fork(wait_all) {
                { clk.next(6); }
            }
            $assert(slow == 1);
            done = 1;
            $finish();
        }
    }
    "#;
    run_pass(code, "test_two_forks");
}
