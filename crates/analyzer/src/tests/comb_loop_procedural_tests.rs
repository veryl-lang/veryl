use super::*;

fn assert_comb_loop(case: &str, code: &str, expected: bool) {
    let errors = analyze(code);
    let actual = errors
        .iter()
        .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. }));
    assert_eq!(actual, expected, "{case}: {errors:?}");
}

#[test]
fn comb_loop_core_semantics_and_region_regressions_2_block_ring_assign_b_c_a_assign_c_b_1() {
    // 2-block ring: assign b = c + a; assign c = b + 1
    let code = r#"
    module ModuleA (
        a: input  logic<8>,
        b: output logic<8>,
        c: output logic<8>,
    ) {
        assign b = c + a;
        assign c = b + 1;
    }
    "#;
    let errors = analyze(code);
    assert!(matches!(errors[0], AnalyzerError::CombinationalLoop { .. }));
}

#[test]
fn comb_loop_core_semantics_and_region_regressions_ff_broken_feedback_assign_x_y_always_ff_y_x_no_loop()
 {
    // FF-broken feedback: assign x = y, always_ff y <= x. No loop.
    let code = r#"
    module ModuleA (
        clk: input  clock,
        a:   input  logic<8>,
        b:   output logic<8>,
    ) {
        var y: logic<8>;
        assign b = y;
        always_ff (clk) {
            y = a;
        }
    }
    "#;
    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn comb_loop_core_semantics_and_region_regressions_disjoint_partial_write_self_reference_a_1_a_0_is_not_a_loop()
 {
    // Disjoint partial-write self-reference: `a[1] = a[0]` is not a loop
    // because the read bit and write bit don't overlap.
    let code = r#"
    module ModuleA {
        var a: logic<2>;
        always_comb {
            a[0] = 0;
        }

        always_comb {
            a[1] = a[0];
        }
    }
    "#;
    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn comb_loop_core_semantics_and_region_regressions_block_local_variables_declared_inside_always_comb_do_not_create()
 {
    // Block-local variables (declared inside `always_comb`) do not create
    // combinational loops under Veryl's blocking semantics.
    let code = r#"
    module ModuleA (
        a: input  logic<32>,
        b: output logic<32>,
    ) {
        always_comb {
            var c: logic<32>;
            var d: logic<32>;
            c = a;
            d = 2 * c;
            c = d;
            b = c;
        }
    }
    "#;
    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn comb_loop_core_semantics_and_region_regressions_continuous_assign_self_reference_real_combinational_loop()
 {
    // Continuous-assign self-reference: real combinational loop.
    let code = r#"
    module ModuleA (
        a: output logic<8>,
    ) {
        assign a = a + 1;
    }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::CombinationalLoop { .. }))
    );
}

#[test]
fn comb_loop_core_semantics_and_region_regressions_conditional_self_reference_one_branch_reads_x_before_assigning()
 {
    // Conditional self-reference: one branch reads x before assigning,
    // synthesizing to `x = cond ? a : (b + x)` which closes the loop.
    let code = r#"
    module ModuleA (
        cond: input  logic,
        a:    input  logic<8>,
        b:    input  logic<8>,
        x:    output logic<8>,
    ) {
        always_comb {
            if cond {
                x = a;
            } else {
                x = b + x;
            }
        }
    }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::CombinationalLoop { .. }))
    );
}

#[test]
fn comb_loop_core_semantics_and_region_regressions_procedural_overwrite_within_always_comb_is_not_a_loop_the_first()
 {
    // Procedural overwrite within always_comb is NOT a loop: the first
    // assignment dominates the second statement's read of `a`.
    let code = r#"
    module ModuleA (
        a: output logic<8>,
    ) {
        always_comb {
            a = 0;
            a = a + 1;
        }
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::CombinationalLoop { .. }))
    );
}

#[test]
fn comb_loop_core_semantics_and_region_regressions_both_branches_assign_x_with_no_self_read_not_a_loop()
 {
    // Both branches assign x with no self-read. Not a loop.
    let code = r#"
    module ModuleA (
        cond: input  logic,
        a:    input  logic<8>,
        b:    input  logic<8>,
        x:    output logic<8>,
    ) {
        always_comb {
            if cond {
                x = a;
            } else {
                x = b;
            }
        }
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::CombinationalLoop { .. }))
    );
}

#[test]
fn comb_loop_core_semantics_and_region_regressions_pre_assign_before_conditional_self_reference_is_dominating_and()
 {
    // Pre-assign before conditional self-reference is dominating and
    // thus NOT a loop. `x = 0` covers all bits before the if/else.
    let code = r#"
    module ModuleA (
        cond: input  logic,
        b:    input  logic<8>,
        x:    output logic<8>,
    ) {
        always_comb {
            x = 0;
            if cond {
                x = b;
            } else {
                x = b + x;
            }
        }
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::CombinationalLoop { .. }))
    );
}

#[test]
fn comb_loop_core_semantics_and_region_regressions_bit_precise_nodekey_distinguishes_ca_0_ca_1_ca_2_so_the()
 {
    // Bit-precise NodeKey distinguishes ca[0]/ca[1]/ca[2] so the
    // forward chain through bit-select inst outputs is not a loop
    // (regression from perlindgren/vips).
    let code = r#"
    module FullAdder (
        a    : input  logic,
        b    : input  logic,
        c    : input  logic,
        sum  : output logic,
        carry: output logic,
    ) {
        assign sum   = a ^ b ^ c;
        assign carry = (a & b) | (c & (a ^ b));
    }

    module Arith (
        a  : input  logic<2>,
        b  : input  logic<2>,
        sub: input  logic   ,
        r  : output logic<2>,
        c  : output logic   ,
    ) {
        var ca: logic<3>;
        assign ca[0] = sub;
        assign c     = ca[2];

        var cl_0: logic;
        var cl_1: logic;
        assign cl_0 = ca[0];
        assign cl_1 = ca[1];

        inst u0: FullAdder (
            a    : a[0]      ,
            b    : b[0] ^ sub,
            c    : cl_0      ,
            sum  : r[0]      ,
            carry: ca[1]     ,
        );
        inst u1: FullAdder (
            a    : a[1]      ,
            b    : b[1] ^ sub,
            c    : cl_1      ,
            sum  : r[1]      ,
            carry: ca[2]     ,
        );
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::CombinationalLoop { .. }))
    );
}

#[test]
fn comb_loop_core_semantics_and_region_regressions_forward_array_chain_c_i_c_i_1_is_not_a_loop_requires()
 {
    // Forward array chain c[i] -> c[i+1] is not a loop -- requires
    // per-element index resolution (regression from celox/linear_sorter).
    let code = r#"
    module Buf (
        i: input  logic<8>,
        o: output logic<8>,
    ) {
        assign o = i;
    }

    module Top (
        d_in:  input  logic<8>,
        d_out: output logic<8>,
    ) {
        var c: logic<8> [3];
        assign c[0] = d_in;
        for i in 0..2 :cell {
            inst u: Buf (
                i: c[i],
                o: c[i + 1],
            );
        }
        assign d_out = c[2];
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::CombinationalLoop { .. }))
    );
}

#[test]
fn comb_loop_const_for_over_many_disjoint_array_elements_is_feed_forward() {
    assert_comb_loop(
        "a constant for loop over many disjoint array elements remains feed-forward",
        r#"
        module Top #(
            param N: u32 = 1024,
        ) (
            mem : input  logic<32> [N],
            i_d : input  logic<32>,
            sum : output logic<32>,
        ) {
            var acc: logic<32> [N];
            always_comb {
                sum = 0;
                for k in 0..N {
                    acc[k] = mem[k] ^ i_d;
                    sum = sum + acc[k];
                }
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_core_semantics_and_region_regressions_bit_disjoint_feedback_through_x_x_a_1_a_0_x_bit()
{
    // Bit-disjoint feedback through `x`: x = a[1], a[0] = x. Bit
    // precision distinguishes bit 0 from bit 1, so not a loop.
    let code = r#"
    module Top (
        b_in: input  logic,
        a:    output logic<2>,
    ) {
        var x: logic;
        var y: logic;
        assign a[0] = x;
        assign a[1] = y;
        assign x    = a[1];
        assign y    = b_in;
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::CombinationalLoop { .. }))
    );
}

#[test]
fn comb_loop_core_semantics_and_region_regressions_dst_side_bit_disjoint_writes_t1_0_0_t1_1_t3_reads_of()
 {
    // Dst-side bit-disjoint writes: `t1[0] = 0; t1[1] = t3;`. Reads of
    // t3 must edge only to t1[1], not t1[0], so no `t1[0]→t2→t3→t1[1]`
    // false cycle through inst feedthrough.
    let code = r#"
    module ModuleAOk2 (
        a: input  logic,
        b: input  logic,
        x: output logic,
    ) {
        always_comb {
            x = a & ~b;
        }
    }

    module ModuleBOk2 (
        a: input  logic,
        y: output logic,
    ) {
        always_comb {
            y = a;
        }
    }

    module ModuleCOk2 (
        a: input  logic,
        y: output logic,
    ) {
        var t1: logic<2>;
        var t2: logic;
        var t3: logic;

        inst mb: ModuleBOk2 (
            a: t1[0],
            y: t2,
        );

        inst ma: ModuleAOk2 (
            a: a,
            b: t2,
            x: t3,
        );

        always_comb {
            t1[0] = 0;
            t1[1] = t3;
            y     = t3;
        }
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::CombinationalLoop { .. })),
        "false positive: {errors:?}"
    );
}

#[test]
fn comb_loop_core_semantics_and_region_regressions_src_side_bit_disjoint_reads_b_a_0_c_a_1_in_one_decl()
 {
    // Src-side bit-disjoint reads: `b = a[0]; c = a[1];` in one decl.
    // The other decl writes `a[1] = b`; per-decl aggregation would
    // incorrectly tie b's flow to a@bit1, closing a false cycle.
    let code = r#"
    module ModuleA (
        d:   input  logic<2>,
        out: output logic,
    ) {
        var a: logic<2>;
        var b: logic;
        var c: logic;

        always_comb {
            b = a[0];
            c = a[1];
        }

        always_comb {
            a[0] = d[0];
            a[1] = b;
        }

        assign out = c;
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::CombinationalLoop { .. })),
        "source-side false positive: {errors:?}"
    );
}

#[test]
fn comb_loop_core_semantics_and_region_regressions_false_positive_cycle_through_unrelated_assigns_in_the_same_comb_block()
 {
    // False-positive cycle through unrelated assigns in the same comb block.
    // `op1_fp32 = op1; op2_fp32 = op2;` are independent.
    // All writes in the same reader_decl are collected as destinations,
    // incorrectly linking op1_fp32 and op2_fp32 and forming a spurious cycle.
    let code = r#"
    module FPComp (
        op1: input logic<32>,
        op2: input logic<32>,

        less_than: output logic,
    ) {
        struct FP32 {
            sign: logic    ,
            exp : logic<8> ,
            frac: logic<23>,
        }

        var op1_fp32: FP32;
        var op2_fp32: FP32;

        always_comb {
            op1_fp32 = op1;
            op2_fp32 = op2;

            if (op1_fp32.exp == op2_fp32.exp) {
                less_than = op1_fp32.frac <: op2_fp32.frac;
            } else {
                less_than = op1_fp32.exp <: op2_fp32.exp;
            }
        }
    }
    "#;
    let errors = analyze(code);
    assert!(errors.is_empty());
}

#[test]
fn comb_loop_statement_order_and_observer_semantics_an_observer_inside_a_writer_process_does_not_create_a_signal_value_edge()
 {
    // An observer inside a writer process does not create a signal-value edge.
    // In particular, IEEE 1800 always_comb sensitivity excludes variables
    // written by the process; observing x[1] must not wire it back to x[0].
    let code = r#"
    module Top (
        a: input  logic,
        o: output logic,
    ) {
        var x: logic<2>;
        var y: logic;
        always_comb {
            x[0] = a;
            $display("x1=%d", x[1]);
            o = x[1];
        }
        assign y = x[0];
        assign x[1] = y;
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "observer-only read formed a false signal cycle: {errors:?}"
    );
}

#[test]
fn comb_loop_statement_order_and_observer_semantics_a_dominating_procedural_write_supplies_the_later_read_memoryssa_must()
 {
    // A dominating procedural write supplies the later read. MemorySSA must
    // resolve it to that definition, not LiveOnEntry(x[0]).
    let code = r#"
    module Top (
        a: input  logic,
        o: output logic,
    ) {
        var x: logic<2>;
        always_comb {
            x[0] = a;
            x[1] = x[0];
            o = x[1];
        }
    }
    "#;
    let errors = analyze(code);
    assert!(
        !errors
            .iter()
            .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "dominating procedural definition was treated as an entry read: {errors:?}"
    );
}

#[test]
fn comb_loop_ordered_module_scope_reassignments_are_feed_forward() {
    assert_comb_loop(
        "module-scope definitions propagate in statement order",
        r#"
        module Top (
            a: output logic,
        ) {
            var b: logic;
            always_comb {
                a = 0;
                b = a;
                a = b;
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_statement_order_and_observer_semantics_the_converse_is_real_after_x_0_consumes_the_entry_value_of_x_1_the()
 {
    // The converse is real: after x[0] consumes the entry value of x[1], the
    // later write makes final x[1] depend on x[0], reducing to x[1] = x[1].
    let code = r#"
    module Top (
        o: output logic<2>,
    ) {
        always_comb {
            o[0] = o[1];
            o[1] = o[0];
        }
    }
    "#;
    let errors = analyze(code);
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "read-before-write feedback was not diagnosed: {errors:?}"
    );
}

#[test]
fn comb_loop_dynamic_select_is_confined_to_static_prefix() {
    // Why this case exists: IEEE 1800-2023 11.5.3 confines buff[0][idx]
    // to the statically selected row. Aliasing it with buff[1] rejects legal,
    // bit-disjoint SystemVerilog and makes the analyzer stronger than the LRM.
    assert_comb_loop(
        "a dynamic suffix aliases only its longest static prefix",
        r#"
        module Top (
            idx: input  logic<2>,
            src: input  logic,
            o  : output logic,
        ) {
            var buff: logic<4, 4>;
            always_comb {
                buff[1] = 0;
                buff[1][0] = src;
            }
            always_comb {
                buff[0][idx] = buff[1][0];
                o = buff[0][0];
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_dynamic_write_is_a_weak_update_a_dynamic_store_cannot_kill_every_candidate_definition()
{
    assert_comb_loop(
        "a dynamic store cannot kill every candidate definition",
        r#"
        module Top (
            idx: input  logic<2>,
            o  : output logic<4>,
        ) {
            always_comb {
                o[idx] = 0;
                o[0] = o[1];
                o[1] = o[0];
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_dynamic_write_is_a_weak_update_a_full_store_still_kills_every_candidate_definition() {
    assert_comb_loop(
        "a full store still kills every candidate definition",
        r#"
        module Top (
            o: output logic<4>,
        ) {
            always_comb {
                o = 0;
                o[0] = o[1];
                o[1] = o[0];
            }
        }
        "#,
        false,
    );
}

fn logical_rhs_side_effect_code(expression: &str) -> String {
    format!(
        r#"
                module Sink (
                    i: input logic,
                ) {{}}
                module Top (
                    o: output logic,
                ) {{
                    var x: logic;
                    function touch (
                        a: input logic,
                    ) -> logic {{
                        x = a;
                        return 0;
                    }}
                    inst u: Sink (
                        i: {expression},
                    );
                    assign o = x;
                }}
                "#
    )
}

#[test]
fn comb_loop_false_logical_and_suppresses_rhs_side_effect() {
    assert_comb_loop(
        "a false logical-and LHS suppresses RHS function side effects",
        &logical_rhs_side_effect_code("1'b0 && touch(o)"),
        false,
    );
}

#[test]
fn comb_loop_true_logical_and_retains_rhs_side_effect() {
    assert_comb_loop(
        "a true logical-and LHS retains RHS function side effects",
        &logical_rhs_side_effect_code("1'b1 && touch(o)"),
        true,
    );
}

#[test]
fn comb_loop_true_logical_or_suppresses_rhs_side_effect() {
    assert_comb_loop(
        "a true logical-or LHS suppresses RHS function side effects",
        &logical_rhs_side_effect_code("1'b1 || touch(o)"),
        false,
    );
}

#[test]
fn comb_loop_false_logical_or_retains_rhs_side_effect() {
    assert_comb_loop(
        "a false logical-or LHS retains RHS function side effects",
        &logical_rhs_side_effect_code("1'b0 || touch(o)"),
        true,
    );
}

#[test]
fn comb_loop_statement_order_and_control_flow_regressions_read_before_write_observes_liveonentry() {
    assert_comb_loop(
        "read before write observes LiveOnEntry",
        r#"
        module Top (
            a: input  logic,
            c: output logic,
        ) {
            var b: logic;
            always_comb {
                c = b;
                b = a;
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_statement_order_and_control_flow_regressions_opposite_directions_on_disjoint_bits() {
    assert_comb_loop(
        "opposite directions on disjoint bits",
        r#"
        module Top (
            i: input  logic<2>,
            o: output logic<2>,
        ) {
            var a: logic<2>;
            var b: logic<2>;
            always_comb {
                a[0] = i[0];
                b[0] = a[0];
                b[1] = i[1];
                a[1] = b[1];
            }
            assign o = {a[1], b[0]};
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_statement_order_and_control_flow_regressions_observer_in_a_writer_with_a_multi_stage_assign_chain()
 {
    assert_comb_loop(
        "observer in a writer with a multi-stage assign chain",
        r#"
        module Top (
            a: input  logic,
            o: output logic,
        ) {
            var x: logic<4>;
            always_comb {
                x[0] = a;
                $display("x3=%d", x[3]);
                o = x[3];
            }
            assign x[1] = x[0];
            assign x[2] = x[1];
            assign x[3] = x[2];
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_statement_order_and_control_flow_regressions_case_phi_with_complete_definitions() {
    assert_comb_loop(
        "case phi with complete definitions",
        r#"
        module Top (
            sel: input  logic<2>,
            a  : input  logic,
            b  : input  logic,
            o  : output logic,
        ) {
            var selected: logic;
            always_comb {
                case sel {
                    2'd0: selected = a;
                    2'd1: selected = b;
                    default: selected = 0;
                }
                o = selected;
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_statement_order_and_control_flow_regressions_one_branch_keeps_an_entry_definition_live()
 {
    assert_comb_loop(
        "one branch keeps an entry definition live",
        r#"
        module Top (
            sel: input  logic,
            o  : output logic,
        ) {
            always_comb {
                if sel {
                    o = 0;
                }
                o = o;
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_statement_order_and_control_flow_regressions_case_without_a_covering_default_keeps_entry_live()
 {
    assert_comb_loop(
        "case without a covering default keeps entry live",
        r#"
        module Top (
            sel: input  logic<2>,
            o  : output logic,
        ) {
            always_comb {
                case sel {
                    2'd0: o = 0;
                    2'd1: o = 1;
                }
                o = o;
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_structural_dependency_semantics_dynamic_same_index_read_write_is_structurally_self_dependent()
 {
    assert_comb_loop(
        "dynamic same-index read/write is structurally self-dependent",
        r#"
        module Top (
            index: input logic<2>,
            o    : output logic,
        ) {
            var values: logic [4];
            always_comb {
                values[index] = values[index];
                o = values[0];
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_structural_dependency_semantics_dynamic_data_write_without_an_old_value_read_is_feed_forward()
 {
    assert_comb_loop(
        "dynamic data write without an old-value read is feed-forward",
        r#"
        module Top (
            index: input  logic<2>,
            data : input  logic,
            o    : output logic,
        ) {
            var values: logic [4];
            always_comb {
                values[index] = data;
                o = values[0];
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_structural_dependency_semantics_a_value_controlling_its_own_dynamic_write_address_closes_a_loop()
 {
    assert_comb_loop(
        "a value controlling its own dynamic write address closes a loop",
        r#"
        module Top (
            data: input  logic,
            o   : output logic,
        ) {
            var index : logic<2>;
            var values: logic [4];
            always_comb {
                index = {1'b0, values[0]};
                values[index] = data;
                o = values[0];
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_mutually_exclusive_reverse_dependencies_are_not_simultaneous_feedback() {
    assert_comb_loop(
        "opposing dependencies on mutually exclusive branches do not form a realizable loop",
        r#"
        module Top (
            sel: input  logic,
            o  : output logic,
        ) {
            var a: logic;
            var b: logic;
            always_comb {
                if sel {
                    a = b;
                } else {
                    b = a;
                }
                o = a | b;
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_break_path_does_not_reach_the_opposing_dependency_after_the_loop_exit() {
    assert_comb_loop(
        "a dependency before break is exclusive with the reverse dependency after it",
        r#"
        module Top (
            stop: input  logic,
            o   : output logic,
        ) {
            var a: logic;
            var b: logic;
            always_comb {
                for _index in 0..1 {
                    if stop {
                        a = b;
                        break;
                    }
                    b = a;
                }
                o = a | b;
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_break_exit_preserves_feedback_killed_only_on_the_continuing_path() {
    assert_comb_loop(
        "a write after a conditional break must not kill the break exit's dependency",
        r#"
        module Top (
            stop: input  logic,
            o   : output logic,
        ) {
            var a: logic;
            var b: logic;
            always_comb {
                for _index in 0..1 {
                    if stop {
                        a = b;
                        break;
                    }
                    a = 0;
                }
                b = a;
                o = b;
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_false_negative_break_condition_controls_a_following_write() {
    // The loop reduces to value = stop ? 0 : 1 while stop = value. The break
    // condition therefore closes a real control-dependency loop.
    assert_comb_loop(
        "a break condition controls whether the following assignment executes",
        r#"
        module Top (
            o: output logic,
        ) {
            var stop: logic;
            var value: logic;
            assign stop = value;
            always_comb {
                value = 0;
                for _index in 0..1 {
                    if stop {
                        break;
                    }
                    value = 1;
                }
                o = value;
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_break_condition_does_not_control_write_after_loop_exit() {
    assert_comb_loop(
        "all break and fallthrough paths execute a write after the loop",
        r#"
        module Top (
            o: output logic,
        ) {
            var stop : logic;
            var value: logic;
            assign stop = value;
            always_comb {
                value = 0;
                for _index in 0..1 {
                    if stop {
                        break;
                    }
                }
                value = 1;
                o = value;
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_branches_in_separate_processes_are_not_mutually_exclusive() {
    assert_comb_loop(
        "branch identities are local to their procedural process",
        r#"
        module Top (
            left : input  logic,
            right: input  logic,
            o    : output logic,
        ) {
            var a: logic;
            var b: logic;
            always_comb {
                if left {
                    a = b;
                } else {
                    a = 0;
                }
            }
            always_comb {
                if right {
                    b = 0;
                } else {
                    b = a;
                }
            }
            always_comb {
                o = a | b;
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_feedback_within_one_branch_is_detected() {
    assert_comb_loop(
        "a realizable loop within one branch remains a loop",
        r#"
        module Top (
            sel: input  logic,
            o  : output logic,
        ) {
            var a: logic;
            var b: logic;
            always_comb {
                if sel {
                    a = b;
                    b = a;
                } else {
                    a = 0;
                    b = 0;
                }
                o = a | b;
            }
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_mutually_exclusive_case_arms_do_not_form_feedback() {
    assert_comb_loop(
        "opposing dependencies in distinct case arms cannot execute together",
        r#"
        module Top (
            sel: input  logic,
            o  : output logic,
        ) {
            var a: logic;
            var b: logic;
            always_comb {
                case sel {
                    1'b0: a = b;
                    default: b = a;
                }
                o = a | b;
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_opposing_shifts_in_mutually_exclusive_arms_are_acyclic() {
    assert_comb_loop(
        "opposing positional dependencies cannot combine across exclusive arms",
        r#"
        module Top (
            sel: input  logic,
            o  : output logic<8>,
        ) {
            var value: logic<8>;
            always_comb {
                if sel {
                    value = value << 1;
                } else {
                    value = value >> 1;
                }
                o = value;
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_structural_dependency_semantics_read_before_write_across_disjoint_nibbles_is_acyclic_without_a_return_pa()
 {
    assert_comb_loop(
        "read-before-write across disjoint nibbles is acyclic without a return path",
        r#"
        module Top (
            o: output logic<8>,
        ) {
            always_comb {
                o[3:0] = o[7:4];
                o[7:4] = 0;
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_structural_dependency_semantics_complete_overwrite_after_a_rotate_shaped_dead_store_kills_the_loop()
 {
    assert_comb_loop(
        "complete overwrite after a rotate-shaped dead store kills the loop",
        r#"
        module Top (
            o: output logic<8>,
        ) {
            always_comb {
                o = {o[3:0], o[7:4]};
                o = 0;
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_structural_dependency_semantics_observer_only_duplicate_reads_do_not_manufacture_value_feedback()
 {
    assert_comb_loop(
        "observer-only duplicate reads do not manufacture value feedback",
        r#"
        module Top (
            a: input  logic,
            o: output logic,
        ) {
            always_comb {
                $display("o=%d %d", o, o);
                o = a;
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_nested_break_exits_only_inner_loop_before_dominating_overwrite() {
    assert_comb_loop(
        "an inner break does not skip the outer loop's dominating overwrite",
        r#"
        module Top (
            o: output logic,
        ) {
            var a: logic;
            var b: logic;
            always_comb {
                for _outer in 0..1 {
                    a = b;
                    for _inner in 0..1 {
                        break;
                    }
                    a = 0;
                }
            }
            assign b = a;
            assign o = b;
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_nested_break_preserves_dependency_before_inner_exit() {
    assert_comb_loop(
        "an inner break retains the reachable dependency before it",
        r#"
        module Top (
            o: output logic,
        ) {
            var a: logic;
            var b: logic;
            always_comb {
                for _outer in 0..1 {
                    for _inner in 0..1 {
                        a = b;
                        break;
                    }
                }
            }
            assign b = a;
            assign o = b;
        }
        "#,
        true,
    );
}

#[test]
fn comb_loop_explicit_self_read_remains_feedback() {
    // Why this case exists: removing entry-state retention edges wholesale
    // would also hide a real source read. The fix must distinguish an implicit
    // preserve path from the explicit `value = value` dependency.
    let errors = analyze(
        r#"
        module Top (
            o: output logic,
        ) {
            var value: logic;
            always_comb {
                value = value;
                o = value;
            }
        }
        "#,
    );
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. })),
        "an explicit same-evaluation self-read remains combinational feedback: {errors:#?}"
    );
}

#[test]
fn comb_loop_if_assignment_to_array_is_feed_forward_if_statement_chain_is_acyclic() {
    // False positive: an always_comb for-loop over cross-coupled arrays that
    // reads index i and writes i+1 (a feed-forward / acyclic CORDIC-style
    // chain) written with an `if`/`else` STATEMENT was wrongly rejected. The
    // condition read `yw[i]` (no assign_target) was wired to every same-array
    // write, forming a false cross-index cycle; it is dominated by the earlier
    // write to `yw[i]`, so the undominated filter now drops it. The
    // byte-identical ternary form was already accepted.
    let if_stmt = r#"
    module Chain (
        x0: input  signed logic<16>,
        y0: input  signed logic<16>,
        xo: output signed logic<16>,
        yo: output signed logic<16>,
    ) {
        var xw: signed logic<16> [5];
        var yw: signed logic<16> [5];
        always_comb {
            xw[0] = x0;
            yw[0] = y0;
            for i in 0..4 {
                if yw[i] <: 0 {
                    xw[i + 1] = xw[i] - (yw[i] >>> i);
                    yw[i + 1] = yw[i] + (xw[i] >>> i);
                } else {
                    xw[i + 1] = xw[i] + (yw[i] >>> i);
                    yw[i + 1] = yw[i] - (xw[i] >>> i);
                }
            }
            xo = xw[4];
            yo = yw[4];
        }
    }
    "#;
    let errors = analyze(if_stmt);
    assert!(
        errors.is_empty(),
        "feed-forward array chain (if-statement form) must not be a comb loop: {errors:?}"
    );
}

#[test]
fn comb_loop_if_assignment_to_array_is_feed_forward_condition_driven_loop_is_reported() {
    // Guard against over-correcting: a genuine condition-driven loop where the
    // condition read is UNDOMINATED (`a` is written only under `if b`, `b` only
    // under `if a`) must still be rejected.
    let real_cond_loop = r#"
    module RealLoop (
        o: output logic,
    ) {
        var a: logic;
        var b: logic;
        always_comb {
            if a {
                b = 1;
            } else {
                b = 0;
            }
            if b {
                a = 1;
            } else {
                a = 0;
            }
            o = a;
        }
    }
    "#;
    let errors = analyze(real_cond_loop);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::CombinationalLoop { .. })),
        "a real condition-driven loop must still be detected: {errors:?}"
    );
}

#[test]
fn comb_loop_if_assignment_to_array_is_feed_forward_array_loop_is_reported() {
    // And a real array loop `m[0] = m[1]; m[1] = m[0]` must still be rejected.
    let real_array_loop = r#"
    module RealArray (
        o: output logic<8>,
    ) {
        var m: logic<8> [2];
        always_comb {
            m[0] = m[1];
            m[1] = m[0];
            o = m[0];
        }
    }
    "#;
    let errors = analyze(real_array_loop);
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, AnalyzerError::CombinationalLoop { .. })),
        "a real array comb loop must still be detected: {errors:?}"
    );
}

#[test]
fn comb_loop_if_expression_arms_are_mutually_exclusive() {
    // `sel` chooses exactly one direction. The opposite edges cannot coexist:
    // true gives state[1] <- state[0], while false gives state[0] <- state[1].
    assert_comb_loop(
        "opposing dependencies in one if expression cannot execute together",
        r#"
        module Top (
            sel: input  logic,
            o  : output logic,
        ) {
            var state: logic<2>;
            always_comb {
                state = if sel ? {state[0], 1'b0} : {1'b0, state[1]};
                o = |state;
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_case_expression_arms_are_mutually_exclusive() {
    // A case expression selects one arm, so neither selected equation contains
    // a cycle even though the two alternatives use opposite directions.
    assert_comb_loop(
        "opposing dependencies in one case expression cannot execute together",
        r#"
        module Top (
            sel: input  logic,
            o  : output logic,
        ) {
            var state: logic<2>;
            always_comb {
                state = case sel {
                    1'b0   : {state[0], 1'b0},
                    default: {1'b0, state[1]},
                };
                o = |state;
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_switch_expression_arms_are_mutually_exclusive() {
    // A switch expression uses only its first matching arm. No execution can
    // combine the left-arm and right-arm dependencies into a cycle.
    assert_comb_loop(
        "opposing dependencies in one switch expression cannot execute together",
        r#"
        module Top (
            left : input  logic,
            right: input  logic,
            o    : output logic,
        ) {
            var state: logic<2>;
            always_comb {
                state = switch {
                    left   : {state[0], 1'b0},
                    right  : {1'b0, state[1]},
                    default: 2'b00,
                };
                o = |state;
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_if_expression_function_side_effects_remain_arm_exclusive() {
    assert_comb_loop(
        "captured writes in opposite expression arms cannot execute together",
        r#"
        module Top (
            sel: input  logic,
            o  : output logic,
        ) {
            var a    : logic;
            var b    : logic;
            var dummy: logic;
            function write_a (value: input logic) -> logic {
                a = value;
                return 1'b0;
            }
            function write_b (value: input logic) -> logic {
                b = value;
                return 1'b0;
            }
            always_comb {
                dummy = if sel ? write_a(b) : write_b(a);
                o = a | b | dummy;
            }
        }
        "#,
        false,
    );
}

#[test]
fn comb_loop_false_negative_runtime_short_circuit_write_kills_disabled_feedback() {
    assert_comb_loop(
        "a conditionally skipped function write cannot kill a realizable loop",
        r#"
        module Top (
            enable: input  logic,
            o     : output logic,
        ) {
            var a    : logic;
            var b    : logic;
            var dummy: logic;
            function clear_a () -> logic {
                a = 1'b0;
                return 1'b0;
            }
            always_comb {
                a = b;
                dummy = enable && clear_a();
                b = a;
                o = a | b | dummy;
            }
        }
        "#,
        true,
    );
}
