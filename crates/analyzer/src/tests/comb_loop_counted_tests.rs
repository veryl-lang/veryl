// Statically counted loops are evaluated once with a symbolic iterator and
// closed as finite recurrences. These cases pin the precision that iteration
// enumeration used to provide and the workloads it could not finish.
use super::*;
use crate::comb_loop_detect::{analysis_size, reset_analysis_size};

fn comb_loops(code: &str) -> Vec<String> {
    analyze(code)
        .into_iter()
        .filter_map(|error| match error {
            AnalyzerError::CombinationalLoop { cycle, .. } => Some(cycle),
            _ => None,
        })
        .collect()
}

fn assert_counted(case: &str, body: &str, expected: bool) {
    let code = format!(
        "package pkg {{ struct Req {{ go: logic, arm: logic, pad: logic<6> }} }}
         module Top (a: input logic<8>, b: input logic<8>, o: output logic<8>, p: output logic<8>) {{ {body} }}"
    );
    let loops = comb_loops(&code);
    assert_eq!(!loops.is_empty(), expected, "{case}: {loops:?}");
    assert!(comb_loop_analysis_is_complete(&code), "{case}");
}

macro_rules! counted_case {
    ($name:ident, $case:literal, $body:literal, $expected:expr) => {
        #[test]
        fn $name() {
            assert_counted($case, $body, $expected);
        }
    };
}

counted_case!(
    counted_array_shift_chain_is_feed_forward,
    "each element is written from its predecessor",
    "var x: logic<8> [5];
     always_comb { x[0] = a; for i in 0..4 { x[i + 1] = x[i]; } o = x[4]; }",
    false
);

counted_case!(
    counted_array_shift_chain_closes_through_an_element,
    "a later element feeds the first one",
    "var x: logic<8> [5]; var f: logic<8>;
     assign f = x[3];
     always_comb { x[0] = f; for i in 0..4 { x[i + 1] = x[i]; } o = x[4]; }",
    true
);

counted_case!(
    counted_array_maps_close_an_elementwise_cycle,
    "two maps read each other's same element",
    "var x: logic<8> [4]; var y: logic<8> [4];
     always_comb { for i in 0..4 { y[i] = x[i]; } }
     always_comb { for i in 0..4 { x[i] = a ^ y[i]; } o = x[0]; }",
    true
);

counted_case!(
    counted_array_maps_with_displaced_elements_are_feed_forward,
    "two maps read each other's preceding element",
    "var x: logic<8> [4]; var y: logic<8> [4];
     always_comb { for i in 0..4 { y[i] = x[i]; } }
     always_comb { x[0] = a; for i in 1..4 { x[i] = y[i - 1]; } o = x[0]; }",
    false
);

counted_case!(
    counted_map_kills_the_previous_array_value,
    "a loop that writes every element overwrites the retained value",
    "var y: logic<8> [4];
     always_comb { for i in 0..4 { y[i] = a; } o = y[2]; for i in 0..4 { y[i] = y[i] ^ b; } }",
    false
);

counted_case!(
    counted_reverse_map_kills_the_previous_array_value,
    "a reverse loop that writes every element overwrites the retained value",
    "var y: logic<8> [4];
     always_comb { for i in rev 0..4 { y[i] = a; } o = y[2]; for i in 0..4 { y[i] = y[i] ^ b; } }",
    false
);

counted_case!(
    counted_strided_map_does_not_kill_skipped_elements,
    "a stepped loop leaves every other element retained",
    "var y: logic<8> [4];
     always_comb { for i in 0..4 step += 2 { y[i] = a; } o = y[1]; for i in 0..4 { y[i] = y[i] ^ b; } }",
    true
);

counted_case!(
    counted_read_before_write_of_the_same_element_is_feedback,
    "each element reads its own previous value",
    "var x: logic<8> [4];
     always_comb { for i in 0..4 { x[i] = x[i] ^ a; } o = x[0]; }",
    true
);

counted_case!(
    counted_wraparound_through_the_first_element_is_feedback,
    "the chain is closed by a write after the loop",
    "var x: logic<8> [4];
     always_comb { for i in 1..4 { x[i] = x[i - 1]; } x[0] = x[3]; o = x[0]; }",
    true
);

counted_case!(
    counted_packed_chain_with_iterator_branch_is_feed_forward,
    "each bit is written from its predecessor or a constant",
    "var x: logic<10>;
     always_comb { for i in 0..10 { if i == 0 { x[i] = 0; } else { x[i] = x[i - 1]; } } o = x[7:0]; }",
    false
);

counted_case!(
    counted_packed_ripple_carry_is_feed_forward,
    "a carry chain over packed bits",
    "var c: logic<9>;
     always_comb {
        c[0] = 0;
        for i in 0..8 { c[i + 1] = a[i] & b[i] | c[i] & (a[i] ^ b[i]); }
        o = c[7:0];
     }",
    false
);

counted_case!(
    counted_packed_chain_closes_through_a_bit,
    "the last bit feeds the first one",
    "var c: logic<9>;
     always_comb { c[0] = c[8]; for i in 0..8 { c[i + 1] = c[i] ^ a[i]; } o = c[7:0]; }",
    true
);

counted_case!(
    counted_cross_coupled_arrays_are_feed_forward,
    "a cordic stage pair with arithmetic and an element condition",
    "var xw: logic<8> [5]; var yw: logic<8> [5];
     always_comb {
        xw[0] = a; yw[0] = b;
        for i in 0..4 {
            if yw[i] <: 0 { xw[i + 1] = xw[i] - (yw[i] >>> i); yw[i + 1] = yw[i] + (xw[i] >>> i); }
            else { xw[i + 1] = xw[i] + (yw[i] >>> i); yw[i + 1] = yw[i] - (xw[i] >>> i); }
        }
        o = xw[4]; p = yw[4];
     }",
    false
);

counted_case!(
    counted_seeded_element_is_overwritten_before_it_is_read,
    "a pre-loop write is replaced before an iteration reads it",
    "var v: logic<8> [3]; var f: logic<8>;
     assign f = v[2];
     always_comb { v = '{default: 0}; v[1] = f; for i in 0..2 { v[i + 1] = v[i]; } o = f; }",
    false
);

counted_case!(
    counted_seeded_element_read_before_it_is_overwritten_is_feedback,
    "the first iteration reads the pre-loop write",
    "var v: logic<8> [3]; var f: logic<8>;
     assign f = v[2];
     always_comb { v = '{default: 0}; v[0] = f; for i in 0..2 { v[i + 1] = v[i]; } o = f; }",
    true
);

counted_case!(
    counted_accumulator_is_feed_forward,
    "a scalar accumulates every element",
    "var s: logic<8>; var x: logic<8> [4];
     always_comb { for i in 0..4 { x[i] = a + i; } }
     always_comb { s = 0; for i in 0..4 { s = s + x[i]; } o = s; }",
    false
);

counted_case!(
    counted_bit_reversal_is_feed_forward,
    "a reversal of input bits",
    "var y: logic<8>;
     always_comb { for i in 0..8 { y[i] = a[7 - i]; } o = y; }",
    false
);

counted_case!(
    counted_nested_loops_are_feed_forward,
    "a transposition through nested loops",
    "var m: logic<8> [4, 4]; var n: logic<8> [4, 4];
     always_comb { for i in 0..4 { for j in 0..4 { n[i][j] = a; } } }
     always_comb { for i in 0..4 { for j in 0..4 { m[i][j] = n[j][i]; } } o = m[1][2]; }",
    false
);

counted_case!(
    counted_nested_map_kills_the_previous_matrix_value,
    "nested loops that write every element overwrite the retained value",
    "var m: logic<8> [4, 4];
     always_comb {
        for i in 0..4 { for j in 0..4 { m[i][j] = a; } }
        o = m[1][2];
        for i in 0..4 { for j in 0..4 { m[i][j] = m[i][j] ^ b; } }
     }",
    false
);

counted_case!(
    counted_call_with_iterator_actual_is_feed_forward,
    "a function maps each element",
    "var x: logic<8> [4];
     function f (v: input logic<8>) -> logic<8> { return v ^ 8'h55; }
     always_comb { x[0] = a; for i in 1..4 { x[i] = f(x[i - 1]); } o = x[3]; }",
    false
);

#[test]
fn counted_loops_do_not_enumerate_large_ranges() {
    // Neither the evaluation size limit nor the iteration count bounds the
    // analysis of a counted loop: the graph does not grow with the count.
    for (case, body, expected) in [
        (
            "shift chain",
            "always_comb { x[0] = a; for i in 0..COUNT { x[i + 1] = x[i]; } o = x[COUNT]; }",
            false,
        ),
        (
            "closed shift chain",
            "always_comb { x[0] = x[COUNT]; for i in 0..COUNT { x[i + 1] = x[i] ^ a; } o = x[0]; }",
            true,
        ),
    ] {
        let mut sizes = Vec::new();
        // The largest array the evaluation size limit admits.
        for count in [4usize, (1 << 20) - 1] {
            let code = format!(
                "module Top (a: input logic, o: output logic) {{ var x: logic [{}]; {} }}",
                count + 1,
                body.replace("COUNT", &count.to_string())
            );
            reset_analysis_size();
            let loops = comb_loops(&code);
            assert_eq!(
                !loops.is_empty(),
                expected,
                "{case}, count={count}: {loops:?}"
            );
            sizes.push(analysis_size());
            assert!(
                comb_loop_analysis_is_complete(&code),
                "{case}, count={count}"
            );
        }
        assert_eq!(
            sizes[0], sizes[1],
            "{case}: the graph grew with the iteration count"
        );
    }
}

#[test]
fn counted_loops_bound_guards_by_body_size() {
    // Enumerating 4096 iterations of a branch made thousands of sequential
    // guards. One symbolic iteration has a single branch.
    for stages in [4, 4096] {
        let code = format!(
            "module Top (s: input logic<{stages}>, i: input logic, o: output logic, independent: output logic) {{
                function gate (s: input logic<{stages}>, x: input logic) -> logic {{
                    var v: logic;
                    v = x;
                    for index in 0..{stages} {{ if s[index] {{ v = !v; }} else {{ v = 0; }} }}
                    return v;
                }}
                assign o = gate(s, i);
                assign independent = independent;
            }}"
        );
        assert!(comb_loop_analysis_is_complete(&code), "stages={stages}");
        assert_eq!(
            comb_loops(&code),
            ["independent -> independent"],
            "stages={stages}"
        );
    }
}

counted_case!(
    counted_strided_array_reads_are_feed_forward,
    "even elements feed the map that writes odd elements",
    "var x: logic<8> [8]; var y: logic<8> [4];
     always_comb { for i in 0..4 { y[i] = x[2 * i]; } }
     always_comb { for i in 0..4 { x[2 * i] = a; x[2 * i + 1] = y[i]; } o = x[1]; }",
    false
);

counted_case!(
    counted_remainder_branches_are_feed_forward,
    "even elements read y and odd elements of y read x",
    "var x: logic [4]; var y: logic [4];
     always_comb { for i in 0..4 { if i % 2 == 0 { x[i] = y[i]; } else { x[i] = a[i]; } } }
     always_comb { for i in 0..4 { if i % 2 == 1 { y[i] = x[i]; } else { y[i] = b[i]; } } }
     assign o = {4'd0, y[0], y[1], y[2], y[3]};",
    false
);

counted_case!(
    counted_remainder_branches_close_through_one_residue,
    "even elements of x and y read each other",
    "var x: logic [4]; var y: logic [4];
     always_comb { for i in 0..4 { if i % 2 == 0 { x[i] = y[i]; } else { x[i] = a[i]; } } }
     always_comb { for i in 0..4 { if i % 2 != 1 { y[i] = x[i]; } else { y[i] = b[i]; } } }
     assign o = {4'd0, y[0], y[1], y[2], y[3]};",
    true
);

counted_case!(
    counted_remainder_branches_by_three_are_feed_forward,
    "elements one past a multiple of three read y, and the others of y read x",
    "var x: logic<6>; var y: logic<6>;
     always_comb { x = 0; for i in 0..6 { if i % 3 == 1 { x[i] = y[i]; } } }
     always_comb { y = 0; for i in 0..6 { if i % 3 != 1 { y[i] = x[i]; } } }
     assign o = {2'd0, y};",
    false
);

counted_case!(
    counted_remainder_branches_keep_the_skipped_elements,
    "odd elements keep the value they had before the loop",
    "var x: logic [4]; var y: logic [4];
     always_comb { x = y; for i in 0..4 { if i % 2 == 0 { x[i] = a[i]; } } }
     assign y = x;
     assign o = {4'd0, x[0], x[1], x[2], x[3]};",
    true
);

counted_case!(
    counted_strided_bits_are_feed_forward,
    "every fourth bit reads y, and the bits after them of y read x",
    "var x: logic<16>; var y: logic<16>;
     always_comb { x = 0; for i in 0..4 { x[i * 4] = y[i * 4]; } }
     always_comb { y = 0; for i in 0..4 { y[i * 4 + 1] = x[i * 4 + 1]; } }
     assign o = y[7:0];",
    false
);

counted_case!(
    counted_strided_bits_close_through_one_stride,
    "every fourth bit of x and of y read each other",
    "var x: logic<16>; var y: logic<16>;
     always_comb { x = 0; for i in 0..4 { x[i * 4] = y[i * 4]; } }
     always_comb { y = 0; for i in 0..4 { y[i * 4] = x[i * 4]; } }
     assign o = y[7:0];",
    true
);

counted_case!(
    counted_inner_iterator_branches_are_feed_forward,
    "the first bit of each row reads y, and the other bits of y read x",
    "var x: logic<16>; var y: logic<16>;
     always_comb { for i in 0..4 { for j in 0..4 { if j == 0 { x[i * 4 + j] = y[i * 4 + j]; } else { x[i * 4 + j] = a[0]; } } } }
     always_comb { for i in 0..4 { for j in 0..4 { if j == 0 { y[i * 4 + j] = b[0]; } else { y[i * 4 + j] = x[i * 4 + j]; } } } }
     assign o = y[7:0];",
    false
);

counted_case!(
    counted_inner_iterator_ranges_are_feed_forward,
    "the first two bits of each row read y, and the others of y read x",
    "var x: logic<16>; var y: logic<16>;
     always_comb { for i in 0..4 { for j in 0..4 { if j <: 2 { x[i * 4 + j] = y[i * 4 + j]; } else { x[i * 4 + j] = a[0]; } } } }
     always_comb { for i in 0..4 { for j in 0..4 { if j <: 2 { y[i * 4 + j] = b[0]; } else { y[i * 4 + j] = x[i * 4 + j]; } } } }
     assign o = y[7:0];",
    false
);

counted_case!(
    counted_inner_iterator_ranges_close_through_one_column,
    "the second bit of each row of x and of y read each other",
    "var x: logic<16>; var y: logic<16>;
     always_comb { for i in 0..4 { for j in 0..4 { if j <: 2 { x[i * 4 + j] = y[i * 4 + j]; } else { x[i * 4 + j] = a[0]; } } } }
     always_comb { for i in 0..4 { for j in 0..4 { if j == 1 { y[i * 4 + j] = x[i * 4 + j]; } else { y[i * 4 + j] = b[0]; } } } }
     assign o = y[7:0];",
    true
);

counted_case!(
    counted_inner_iterator_elements_are_feed_forward,
    "the first element of each row reads y, and the others of y read x",
    "var x: logic [4, 4]; var y: logic [4, 4];
     always_comb { for i in 0..4 { for j in 0..4 { if j == 0 { x[i][j] = y[i][j]; } else { x[i][j] = a[0]; } } } }
     always_comb { for i in 0..4 { for j in 0..4 { if j == 0 { y[i][j] = b[0]; } else { y[i][j] = x[i][j]; } } } }
     assign o = {7'd0, y[0][0]};",
    false
);

counted_case!(
    counted_reversed_arrays_are_feed_forward,
    "two reversals form a chain from the first element",
    "var x: logic<8> [4]; var y: logic<8> [4];
     always_comb { for i in 0..4 { y[i] = x[3 - i]; } }
     always_comb { x[0] = a; for i in 1..4 { x[i] = y[4 - i]; } o = x[3]; }",
    false
);

counted_case!(
    counted_reversed_bits_are_feed_forward,
    "two bit reversals form a chain from the first bit",
    "var x: logic<8>; var y: logic<8>;
     always_comb { for i in 0..8 { y[i] = x[7 - i]; } }
     always_comb { x[0] = a[0]; for i in 1..8 { x[i] = y[8 - i]; } o = x; }",
    false
);

counted_case!(
    counted_array_to_packed_is_feed_forward,
    "array elements and packed bits are displaced across axes",
    "var y: logic<4>; var r: logic [4];
     always_comb { for i in 0..4 { y[i] = r[i]; } }
     always_comb { r[0] = a[0]; for i in 1..4 { r[i] = y[i - 1]; } o = {4'd0, y}; }",
    false
);

counted_case!(
    counted_bit_condition_is_elementwise,
    "a bit condition selects each element from its own bit",
    "var s: logic<8>; var y: logic<8>;
     assign s = {y[6:0], a[0]};
     always_comb { for i in 0..8 { if s[i] { y[i] = b[i]; } else { y[i] = 0; } } o = y; }",
    false
);

counted_case!(
    counted_wide_strided_lanes_are_feed_forward,
    "each wide element reads its own lane, and only a later lane is fed back",
    "var x: logic<256>; var y: logic<128> [2];
     assign x = {y[0], 120'd0, a};
     always_comb { for i in 0..2 { y[i] = x[i * 128 +: 128]; } }
     assign o = y[1][7:0];",
    false
);

counted_case!(
    counted_wide_strided_lanes_close_through_their_own_lane,
    "an element reads the lane that it feeds",
    "var x: logic<256>; var y: logic<128> [2];
     assign x = {y[1], 120'd0, a};
     always_comb { for i in 0..2 { y[i] = x[i * 128 +: 128]; } }
     assign o = y[1][7:0];",
    true
);

counted_case!(
    counted_strided_struct_members_are_feed_forward,
    "each response bit reads one member of its own request",
    "var req: pkg::Req<2>; var rsp: logic<2> [2];
     for bank in 0..2 :g_bank {
         assign req[bank] = pkg::Req'{ go: ~rsp[bank][0], arm: a[bank], pad: 6'd0 };
     }
     always_comb { for i in 0..2 { rsp[i][1] = req[i].go; rsp[i][0] = req[i].arm; } }
     assign o = {6'd0, rsp[0][1], rsp[1][0]};",
    false
);

#[test]
fn counted_struct_member_array_boundary() {
    let code = r#"
    package slice_pkg {
        struct Req {
            go : logic      ,
            arm: logic      ,
            pad: logic<2048>,
        }
    }

    module Core (
        rsp_i: input  logic<2>      ,
        en   : input  logic         ,
        req_o: output slice_pkg::Req,
    ) {
        assign req_o = slice_pkg::Req'{
            go : ~rsp_i[0],
            arm: en       ,
            pad: 2048'd0  ,
        };
    }

    module Macro (
        req_i: input  slice_pkg::Req<2>,
        rsp_o: output logic<2>      [2],
    ) {
        always_comb {
            for i in 0..2 {
                rsp_o[i][1] = req_i[i].go;
                rsp_o[i][0] = req_i[i].arm;
            }
        }
    }

    module DeepTop (
        en: input  logic,
        q : output logic,
    ) {
        var req: slice_pkg::Req<2>;
        var rsp: logic<2>      [2];

        for bank in 0..2 :g_bank {
            inst u: Core (
                rsp_i: rsp[bank],
                en              ,
                req_o: req[bank],
            );
        }
        inst m: Macro (
            req_i: req,
            rsp_o: rsp,
        );

        assign q = rsp[0][1] ^ rsp[1][0];
    }

    module ShallowTop (
        rsp_i: input  logic<2>      ,
        en   : input  logic         ,
        req_o: output slice_pkg::Req,
    ) {
        inst u: Core (
            rsp_i,
            en   ,
            req_o,
        );
    }
    "#;
    let loops = comb_loops(code);
    assert!(loops.is_empty(), "{loops:?}");
}

counted_case!(
    counted_packed_chain_from_the_first_bit_closes_through_feedback,
    "the first bit reads the last bit through a chain from the first bit",
    "var x: logic<10>; var f: logic;
     assign f = x[9];
     always_comb { for i in 0..10 { if i == 0 { x[i] = f; } else { x[i] = x[i - 1]; } } o = x[7:0]; }",
    true
);

counted_case!(
    counted_packed_chain_with_an_unrelated_branch_is_feed_forward,
    "each bit is written from its predecessor or a constant",
    "var x: logic<10>;
     always_comb { for i in 0..10 { if a[0] { x[i] = 0; } else { x[i] = x[i - 1]; } } o = x[7:0]; }",
    false
);

counted_case!(
    counted_array_chain_from_the_first_element_closes_through_feedback,
    "the first element reads the last element through a chain from the first element",
    "var x: logic<8> [4]; var f: logic<8>;
     assign f = x[3];
     always_comb { for i in 0..4 { x[i] = if i == 0 ? f : x[i - 1]; } o = x[0]; }",
    true
);

counted_case!(
    counted_seeded_element_is_overwritten_by_a_map_from_the_first_element,
    "a loop writing from the first element replaces a pre-loop write before reading it",
    "var v: logic<8> [3]; var f: logic<8>;
     assign f = v[2];
     always_comb { v = '{default: 0}; v[1] = f; for i in 0..3 { v[i] = if i == 0 ? a : v[i - 1]; } o = f; }",
    false
);

#[test]
fn counted_iterator_branches_write_only_their_iterations() {
    // A branch on the iterator against a constant confines each side to the
    // iterations that take it, as enumerating the iterations did.
    for (case, body, expected) in [
        (
            "equal",
            "for i in 0..5 { y[i] = if i == 0 ? o : a; } p = y[1];",
            false,
        ),
        (
            "equal reaches its element",
            "for i in 0..5 { y[i] = if i == 0 ? o : a; } p = y[0];",
            true,
        ),
        (
            "unequal",
            "for i in 0..5 { y[i] = if i != 0 ? a : o; } p = y[1];",
            false,
        ),
        (
            "interior equal leaves the other iterations",
            "for i in 0..5 { y[i] = if i == 2 ? a : o; } p = y[4];",
            true,
        ),
        (
            "less",
            "for i in 0..5 { y[i] = if i <: 2 ? o : a; } p = y[3];",
            false,
        ),
        (
            "less or equal",
            "for i in 0..5 { y[i] = if i <= 2 ? o : a; } p = y[2];",
            true,
        ),
        (
            "greater",
            "for i in 0..5 { y[i] = if i >: 2 ? o : a; } p = y[2];",
            false,
        ),
        (
            "greater or equal",
            "for i in 0..5 { y[i] = if i >= 2 ? o : a; } p = y[1];",
            false,
        ),
        (
            "constant on the left",
            "for i in 0..5 { y[i] = if 2 <: i ? o : a; } p = y[2];",
            false,
        ),
        (
            "negated statement branch",
            "for i in 0..5 { if !(i == 0) { y[i] = o; } else { y[i] = a; } } p = y[0];",
            false,
        ),
        (
            "reverse",
            "for i in rev 0..5 { y[i] = if i == 4 ? o : a; } p = y[3];",
            false,
        ),
        (
            "outer iterator",
            "for i in 0..2 { for j in 0..2 { w[i][j] = if i == 0 ? o : a; } } p = w[1][0];",
            false,
        ),
        (
            "packed",
            "for i in 0..5 { z[i] = if i == 0 ? o[0] : a[0]; } p = {7'b0, z[1]};",
            false,
        ),
        (
            "runtime conjunct keeps the branch",
            "for i in 0..5 { if i == 0 && b[0] { y[i] = o; } else { y[i] = a; } } p = y[0];",
            true,
        ),
    ] {
        let code = format!(
            "module Top (a: input logic<8>, b: input logic<8>, o: output logic<8>) {{
                var y: logic<8> [5]; var z: logic<5>; var w: logic<8> [2, 2]; var p: logic<8>;
                always_comb {{ {body} }}
                assign o = p;
            }}"
        );
        let loops = comb_loops(&code);
        assert_eq!(!loops.is_empty(), expected, "{case}: {loops:?}");
        assert!(comb_loop_analysis_is_complete(&code), "{case}");
    }
}

#[test]
fn counted_scalar_writes_reach_only_their_positions() {
    // A scalar written through an iterator-affine index reaches only the
    // positions the index takes, including the gaps between its steps.
    for (case, body, expected) in [
        (
            "column",
            "for i in 0..2 { w[i][1] = o; } p = w[1][0];",
            false,
        ),
        (
            "column reaches its element",
            "for i in 0..2 { w[i][1] = o; } p = w[1][1];",
            true,
        ),
        ("stride", "for i in 0..2 { y[2 * i] = o; } p = y[1];", false),
        (
            "stride reaches its element",
            "for i in 0..2 { y[2 * i] = o; } p = y[2];",
            true,
        ),
        (
            "descending stride",
            "for i in 0..2 { y[4 - 2 * i] = o; } p = y[3];",
            false,
        ),
        (
            "inner iterator branch",
            "for i in 0..2 { for j in 0..2 { w[i][j] = if j == 1 ? o : a; } } p = w[1][0];",
            false,
        ),
        (
            "inner iterator branch reaches its element",
            "for i in 0..2 { for j in 0..2 { w[i][j] = if j == 1 ? o : a; } } p = w[0][1];",
            true,
        ),
        (
            "remainder branch",
            "for i in 0..5 { y[i] = if i % 2 == 1 ? o : a; } p = y[2];",
            false,
        ),
        (
            "remainder branch reaches its element",
            "for i in 0..5 { y[i] = if i % 2 == 1 ? o : a; } p = y[3];",
            true,
        ),
    ] {
        let code = format!(
            "module Top (a: input logic<8>, o: output logic<8>) {{
                var y: logic<8> [5]; var w: logic<8> [2, 2]; var p: logic<8>;
                always_comb {{ {body} }}
                assign o = p;
            }}"
        );
        let loops = comb_loops(&code);
        assert_eq!(!loops.is_empty(), expected, "{case}: {loops:?}");
        assert!(comb_loop_analysis_is_complete(&code), "{case}");
    }
}

#[test]
fn counted_wrapped_index_selects_the_element_it_wraps_to() {
    // An index below zero in its own width wraps to an unsigned value. It
    // selects nothing only when that value lies past the last element.
    for (case, index, expected) in [
        ("narrow wrap stays in range", "(i as 2) - 2'd1", true),
        ("narrow operand wraps in range", "k - 2'd1", true),
        ("wide wrap leaves the range", "i - 1", false),
    ] {
        let code = format!(
            "module Top (a: input logic, k: input logic, o: output logic) {{
                var x: logic [4]; var y: logic [2];
                assign x[0] = a; assign x[1] = a; assign x[2] = a; assign x[3] = y[0];
                always_comb {{ for i in 0..2 {{ y[i] = x[{index}]; }} }}
                assign o = y[1];
            }}"
        );
        let loops = comb_loops(&code);
        assert_eq!(!loops.is_empty(), expected, "{case}: {loops:?}");
        assert!(comb_loop_analysis_is_complete(&code), "{case}");
    }
}
