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

#[test]
fn counted_static_signed_operands_extend_by_their_context() {
    // A static signed operand is sign-extended in a signed context and
    // zero-extended in an unsigned one, both in an index and in a branch on
    // the iterator.
    for (case, body, expected) in [
        (
            "signed index context",
            "always_comb { for i in 0..32 { z[i] = if i == 7 ? q[0] : a; } }
             always_comb { for i in 0..2 { q[i] = z[i + S + 8]; } }",
            true,
        ),
        (
            "unsigned index context",
            "always_comb { for i in 0..32 { z[i] = if i == 23 ? q[0] : a; } }
             always_comb { for i in 0..2 { q[i] = z[i + S + 8'd8]; } }",
            true,
        ),
        (
            "unsigned index context skips the sign-extended element",
            "always_comb { for i in 0..32 { z[i] = if i == 7 ? q[0] : a; } }
             always_comb { for i in 0..2 { q[i] = z[i + S + 8'd8]; } }",
            false,
        ),
        (
            "a negative constant equals no iteration",
            "always_comb { for i in 0..16 { z[i] = if i == S ? a : q[0]; } }
             always_comb { for i in 0..2 { q[i] = z[15]; } }",
            true,
        ),
    ] {
        let code = format!(
            "module Top (a: input logic, o: output logic) {{
                const S: signed logic<4> = 4'b1111;
                var z: logic [32]; var q: logic [2];
                {body}
                assign o = q[0] ^ q[1];
            }}"
        );
        let loops = comb_loops(&code);
        assert_eq!(!loops.is_empty(), expected, "{case}: {loops:?}");
        assert!(comb_loop_analysis_is_complete(&code), "{case}");
    }
}

#[test]
fn counted_concatenated_bit_select_receives_its_own_part() {
    // A bit selected by the iterator inside a concatenated destination
    // receives only its own part of the value and only the bits it takes.
    for (case, body, expected) in [
        (
            "its own part",
            "for i in 0..4 { {x[i], y} = {f, a}; }",
            true,
        ),
        (
            "the other part",
            "for i in 0..4 { {x[i], y} = {a, f}; }",
            false,
        ),
        (
            "an unwritten bit",
            "for i in 0..3 { {x[i], y} = {f, a}; }",
            false,
        ),
    ] {
        let code = format!(
            "module Top (a: input logic, o: output logic<5>) {{
                var x: logic<4>; var y: logic; var f: logic;
                always_comb {{ x = 0; y = 0; {body} }}
                assign f = x[3];
                assign o = {{x, y}};
            }}"
        );
        let loops = comb_loops(&code);
        assert_eq!(!loops.is_empty(), expected, "{case}: {loops:?}");
        assert!(comb_loop_analysis_is_complete(&code), "{case}");
    }
}

#[test]
fn counted_strided_writes_do_not_grow_with_the_iteration_count() {
    // A strided write keeps the positions between its steps, without one
    // projection per position.
    for (case, template) in [
        (
            "elements",
            "var x: logic [SIZE]; always_comb { for i in 0..COUNT { x[i * 2] = a; x[i * 2 + 1] = b; } o = x[0]; }",
        ),
        (
            "packed slices",
            "var x: logic<SIZE>; always_comb { x = 0; for i in 0..COUNT { x[i * 4 +: 2] = {a, b}; } o = x[0]; }",
        ),
    ] {
        let mut sizes = Vec::new();
        for count in [1usize << 8, 1 << 12] {
            let body = template
                .replace("SIZE", &(count * 4).to_string())
                .replace("COUNT", &count.to_string());
            let code = format!(
                "module Top (a: input logic, b: input logic, o: output logic) {{ {body} }}"
            );
            reset_analysis_size();
            assert!(comb_loops(&code).is_empty(), "{case}, count={count}");
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
fn counted_strided_bit_write_reaches_only_its_bits() {
    // A scalar written to an iterator-affine bit reaches only the bits the
    // select takes, not the bits between its steps.
    for (case, body, expected) in [
        (
            "a gap bit",
            "for i in 0..4 { v[4 * i] = o; } p = v[1];",
            false,
        ),
        (
            "a written bit",
            "for i in 0..4 { v[4 * i] = o; } p = v[4];",
            true,
        ),
        (
            "a gap bit of a descending stride",
            "for i in 0..4 { v[12 - 4 * i] = o; } p = v[5];",
            false,
        ),
        (
            "a gap bit with a source moving along",
            "for i in 0..4 { v[4 * i] = s[i] ^ o; } p = v[2];",
            false,
        ),
    ] {
        let code = format!(
            "module Top (s: input logic<4>, o: output logic) {{
                var v: logic<16>; var p: logic;
                always_comb {{ v = 0; {body} }}
                assign o = p;
            }}"
        );
        let loops = comb_loops(&code);
        assert_eq!(!loops.is_empty(), expected, "{case}: {loops:?}");
        assert!(comb_loop_analysis_is_complete(&code), "{case}");
    }
}

#[test]
fn counted_non_affine_iterator_uses_take_each_iteration() {
    // An iterator used other than affinely selects positions a symbolic
    // iteration cannot follow, so the loop takes each iteration's value.
    for (case, body, expected) in [
        (
            "a shifted index is feed-forward",
            "always_comb { a[0] = s[0]; for i in 1..4 { a[i] = a[i >> 1]; } }
             assign o = a[3];",
            false,
        ),
        (
            "a shifted index closes through its root",
            "always_comb { a[0] = o; for i in 1..4 { a[i] = a[i >> 1]; } }
             assign o = a[3];",
            true,
        ),
        (
            "a case on the iterator",
            "always_comb { for i in 0..4 { case i { 0: a[i] = o; default: a[i] = s[0]; } } }
             assign o = a[1];",
            false,
        ),
        (
            "a squared index",
            "always_comb { for i in 0..10 { b[i] = if i == 9 ? o : s[0]; } }
             always_comb { for i in 0..3 { a[i] = b[i * i]; } }
             assign o = a[2];",
            false,
        ),
    ] {
        let code = format!(
            "module Top (s: input logic<4>, o: output logic) {{
                var a: logic [4]; var b: logic [10];
                {body}
            }}"
        );
        let loops = comb_loops(&code);
        assert_eq!(!loops.is_empty(), expected, "{case}: {loops:?}");
        assert!(comb_loop_analysis_is_complete(&code), "{case}");
    }
}

#[test]
fn counted_step_takes_only_its_values() {
    for (case, read, expected) in [
        ("a skipped element", "y[1]", false),
        ("a taken element", "y[2]", true),
    ] {
        let code = format!(
            "module Top (o: output logic) {{
                var y: logic [8];
                always_comb {{ y = '{{default: 0}}; for i in 0..8 step += 2 {{ y[i] = o; }} }}
                assign o = {read};
            }}"
        );
        let loops = comb_loops(&code);
        assert_eq!(!loops.is_empty(), expected, "{case}: {loops:?}");
        assert!(comb_loop_analysis_is_complete(&code), "{case}");
    }
}

counted_case!(
    counted_heap_tree_reads_children_written_by_earlier_iterations,
    "each node of a heap-indexed adder tree reads its children, written earlier or by the leaves",
    "var node: logic<8> [16];
     always_comb {
         for i in 0..8 { node[8 + i] = a; }
         for i in rev 1..8 { node[i] = node[2 * i] + node[2 * i + 1]; }
         o = node[1];
     }",
    false
);

counted_case!(
    counted_inner_loop_folds_an_element_written_by_the_outer_iteration,
    "each element is copied from its predecessor and then folded in place",
    "var c_tap: logic<8> [5];
     always_comb {
         c_tap[0] = a;
         for b in 0..4 {
             c_tap[b + 1] = c_tap[b];
             for i in 0..8 {
                 c_tap[b + 1] = (c_tap[b + 1] >> 1) ^ ({ (c_tap[b + 1][0] ^ a[i]) repeat 8 });
             }
         }
         o = c_tap[4];
     }",
    false
);

counted_case!(
    counted_inner_loop_accumulates_into_an_element_cleared_first,
    "each element is cleared and then accumulated by an inner loop",
    "var pcnt: logic<4> [16]; var ip: logic<4>;
     always_comb {
         for i in 0..16 {
             ip = i as 4;
             pcnt[i] = '0;
             for b in 0..4 { pcnt[i] = pcnt[i] + ip[b]; }
         }
         o = pcnt[3];
     }",
    false
);

counted_case!(
    counted_bit_written_in_an_iteration_feeds_the_next_state,
    "a scrambler bit is written and then shifted into the next state",
    "var st: logic<8> [9]; var scr: logic<8>;
     always_comb {
         st[0] = a;
         for i in 0..8 {
             scr[i] = b[i] ^ st[i][3] ^ st[i][7];
             st[i + 1] = {st[i][6:0], scr[i]};
         }
         o = scr;
     }",
    false
);

counted_case!(
    counted_reversed_bit_written_in_an_iteration_feeds_the_next_state,
    "a scrambler bit at a descending position feeds the next state",
    "var st: logic<8> [9]; var scr: logic<8>;
     always_comb {
         st[0] = a;
         for j in 0..8 {
             scr[7 - j] = b[7 - j] ^ st[j][3] ^ st[j][7];
             st[j + 1] = {st[j][6:0], scr[7 - j]};
         }
         o = scr;
     }",
    false
);

counted_case!(
    counted_element_rewritten_in_the_same_iteration_reads_its_new_value,
    "an element is written from its predecessor and then updated in place",
    "var x: logic<8> [5];
     always_comb { x[0] = a; for i in 1..5 { x[i] = x[i - 1]; x[i] = x[i] ^ b; } o = x[4]; }",
    false
);

counted_case!(
    counted_initialized_elements_read_ahead_without_feedback,
    "every element is written before a loop reads its successor",
    "var x: logic<8> [8];
     always_comb {
         for i in 0..8 { x[i] = a; }
         for i in 1..7 { x[i] = x[i - 1] ^ x[i + 1]; }
         o = x[3];
     }",
    false
);

counted_case!(
    counted_uninitialized_elements_read_ahead_close_a_loop,
    "an element reads its successor before the loop writes it",
    "var x: logic<8> [8];
     always_comb { x[0] = a; x[7] = b; for i in 1..7 { x[i] = x[i - 1] ^ x[i + 1]; } o = x[3]; }",
    true
);

counted_case!(
    counted_conditional_update_keeps_the_unconditional_write,
    "a conditional update reads the element written before it",
    "var x: logic<8> [4];
     always_comb { for i in 0..4 { x[i] = a; if b[i] { x[i] = x[i] ^ b; } } o = x[1]; }",
    false
);

counted_case!(
    counted_both_arms_write_before_a_read,
    "both arms of a branch write the element that is read after it",
    "var x: logic<8> [4]; var y: logic<8> [4];
     always_comb { for i in 0..4 { if b[i] { x[i] = a; } else { x[i] = b; } y[i] = x[i]; } o = y[1]; }",
    false
);

counted_case!(
    counted_iterator_branch_chains_from_the_first_element,
    "the first iteration writes the head and the others read the previous element",
    "var x: logic<8> [4];
     always_comb { for i in 0..4 { if i == 0 { x[i] = a; } else { x[i] = x[i - 1] ^ b; } } o = x[3]; }",
    false
);

counted_case!(
    counted_inner_loop_chains_along_each_row,
    "each row starts from an input and chains along its columns",
    "var x: logic<8> [4, 4];
     always_comb { for i in 0..4 { x[i][0] = a; for j in 1..4 { x[i][j] = x[i][j - 1] ^ b; } } o = x[2][3]; }",
    false
);

counted_case!(
    counted_rows_chain_from_the_previous_row,
    "each row reads the row the previous outer iteration wrote",
    "var x: logic<8> [4, 4];
     always_comb {
         for j in 0..4 { x[0][j] = a; }
         for i in 1..4 { for j in 0..4 { x[i][j] = x[i - 1][j] ^ b; } }
         o = x[3][2];
     }",
    false
);

counted_case!(
    counted_chain_closes_through_an_element_written_after_the_loop_reads_it,
    "the head of a chain reads its tail",
    "var x: logic<8> [4]; var y: logic<8>;
     assign y = x[3];
     always_comb { x[0] = y; for i in 1..4 { x[i] = x[i - 1]; } o = x[3]; }",
    true
);

counted_case!(
    counted_multiplied_step_takes_only_its_iterations,
    "a multiplied step skips the values between its iterations",
    "var x: logic [9];
     always_comb {
         x[3] = a[0];
         x[0] = x[4];
         for i in 1..9 step *= 2 { x[i] = x[i - 1]; }
         o = {7'd0, x[8]};
     }",
    false
);

counted_case!(
    counted_multiplied_step_keeps_its_loops,
    "a multiplied step relates each iteration to its predecessor",
    "var x: logic [9];
     always_comb {
         x[0] = x[4];
         for i in 1..9 step *= 2 { x[i] = x[i >> 1]; }
         o = {7'd0, x[8]};
     }",
    true
);

counted_case!(
    counted_iterator_repeat_count_takes_each_iteration,
    "an iterator repeat count sets the positions each iteration writes",
    "var u: logic<6>;
     always_comb {
         u[4] = u[1];
         for i in 2..3 { u[3:0] = {u[5:4] repeat i}; }
         o = {2'd0, u};
     }",
    false
);

/// Whether a counted loop whose iterators divide into `n` progressions or
/// branch arms completes within `limit` units of procedure work.
fn divided_loop_completes(case: &str, n: usize, limit: usize) -> bool {
    let body = match case {
        // The positions of one `i` form a progression over `j`, but the
        // progressions of different `i` interleave.
        "product" => format!(
            "var x: logic [{}];
             always_comb {{ for i in 0..{n} {{ for j in 0..{n} {{ x[i * {} + j * 3] = a[0]; }} }} o = {{7'd0, x[5]}}; }}",
            n * (n + 1) * 3,
            n + 1
        ),
        // Every remainder is its own arm.
        _ => format!(
            "var x: logic [{}];
             always_comb {{ for i in 0..{} {{ if i % {n} == 0 {{ x[i] = a[0]; }} else {{ x[i] = a[1]; }} }} o = {{7'd0, x[5]}}; }}",
            n * 4,
            n * 4
        ),
    };
    let code = format!(
        "module Top (a: input logic<8>, b: input logic<8>, o: output logic<8>) {{ {body} }}"
    );
    crate::comb_loop_detect::with_procedure_guard_limit(limit, || {
        comb_loop_analysis_is_complete(&code)
    })
}

#[test]
fn counted_iterator_divisions_are_charged_to_the_procedure_work() {
    for case in ["product", "remainder"] {
        assert!(divided_loop_completes(case, 2, 64), "{case}");
        assert!(divided_loop_completes(case, 64, 1 << 20), "{case}");
        assert!(!divided_loop_completes(case, 64, 64), "{case}");
    }
}

counted_case!(
    counted_iterator_values_beyond_the_size_limit_stay_conservative,
    "a body that needs its iterator's values beyond the size limit reads whole",
    "var y: logic [4];
     always_comb { for i in 0..4194304 { y[i >> 20] = y[1]; } o = {7'd0, y[0]}; }",
    true
);

counted_case!(
    counted_scalar_reads_reach_only_their_iterations,
    "a scalar reads only the elements its iterations take",
    "var x: logic [4];
     var c: logic;
     always_comb {
         for i in 0..2 { c = x[i]; }
         x[2] = c;
         for i in 3..4 { c = x[i]; }
     }
     assign o = {7'd0, c};",
    false
);

counted_case!(
    counted_scalar_reads_keep_the_elements_they_take,
    "a scalar that reads the element it feeds closes a loop",
    "var x: logic [4];
     var c: logic;
     always_comb {
         for i in 0..4 { c = x[i]; }
         x[3] = c;
     }
     assign o = {7'd0, c};",
    true
);

counted_case!(
    counted_iterator_inequality_excludes_its_value,
    "the side that differs from an inner value never takes it",
    "var x: logic [4];
     var c: logic;
     always_comb {
         for i in 0..4 { if i == 2 { x[2] = c; } else { c = x[i]; } }
     }
     assign o = {7'd0, c};",
    false
);

counted_case!(
    counted_strided_scalar_reads_skip_the_elements_between,
    "a strided scalar read takes none of the elements between its positions",
    "var x: logic [4];
     var c: logic;
     always_comb {
         for i in 0..2 { c = x[2 * i]; }
         x[1] = c;
         c = x[3];
     }
     assign o = {7'd0, c};",
    false
);

counted_case!(
    counted_strided_scalar_reads_keep_their_positions,
    "a strided scalar read that feeds one of its positions closes a loop",
    "var x: logic [4];
     var c: logic;
     always_comb {
         for i in 0..2 { c = x[2 * i]; }
         x[2] = c;
         c = x[3];
     }
     assign o = {7'd0, c};",
    true
);

counted_case!(
    counted_overlapping_scalar_reads_reach_only_their_positions,
    "overlapping progressions of a scalar read stay within their positions",
    "var x: logic [4];
     var c: logic;
     always_comb {
         for i in 0..2 { for j in 0..2 { c = x[i + j]; } }
         x[3] = c;
         c = x[0];
     }
     assign o = {7'd0, c};",
    false
);

counted_case!(
    counted_overlapping_scalar_reads_keep_their_positions,
    "overlapping progressions of a scalar read keep every position they take",
    "var x: logic [4];
     var c: logic;
     always_comb {
         for i in 0..2 { for j in 0..2 { c = x[i + j]; } }
         x[2] = c;
         c = x[0];
     }
     assign o = {7'd0, c};",
    true
);

counted_case!(
    counted_in_place_write_reaches_a_later_iteration_read,
    "a read on later iterations sees an earlier iteration's write in place",
    "var x: logic [4];
     var y: logic [4];
     var c: logic;
     always_comb {
         c = 0;
         for i in 0..2 {
             if i == 1 { c = y[0]; }
             y[0] = x[0] ^ c;
         }
     }
     assign o = {7'd0, c};",
    false
);

counted_case!(
    counted_in_place_write_misses_a_first_iteration_read,
    "a read on the first iteration sees the value from before the loop",
    "var x: logic [4];
     var y: logic [4];
     var c: logic;
     always_comb {
         c = 0;
         for i in 0..2 {
             if i == 0 { c = y[0]; }
             y[0] = x[0] ^ c;
         }
     }
     assign o = {7'd0, c};",
    true
);

counted_case!(
    counted_in_place_write_on_an_earlier_iteration_reaches_later_reads,
    "a write on the first iteration is what later iterations read",
    "var x: logic [4];
     var y: logic [4];
     var c: logic;
     always_comb {
         c = 0;
         for i in 0..2 {
             if i == 0 { y[0] = x[0] ^ c; }
             c = y[0];
         }
     }
     assign o = {7'd0, c};",
    false
);

counted_case!(
    counted_enumerated_inner_loop_reaches_a_later_iteration_read,
    "a later iteration reads what an inner loop over its values wrote",
    "var x: logic [4];
     var y: logic [4];
     var c: logic;
     always_comb {
         c = 0;
         for i in 0..2 {
             if i == 1 { c = y[i]; }
             for j in 0..4 { y[j] = x[j >> 1] ^ c; }
         }
     }
     assign o = {7'd0, c};",
    false
);

counted_case!(
    counted_enumerated_inner_loop_misses_a_first_iteration_read,
    "the first iteration reads the value from before an inner loop over its values",
    "var x: logic [4];
     var y: logic [4];
     var c: logic;
     always_comb {
         c = 0;
         for i in 0..2 {
             if i == 0 { c = y[i]; }
             for j in 0..4 { y[j] = x[j >> 1] ^ c; }
         }
     }
     assign o = {7'd0, c};",
    true
);

counted_case!(
    counted_multiplied_inner_loop_reaches_a_later_iteration_read,
    "a later iteration reads what an inner loop over multiplied values wrote",
    "var x: logic [9];
     var y: logic [4];
     var c: logic;
     always_comb {
         c = 0;
         for i in 0..2 {
             if i == 1 { c = y[i]; }
             for j in 1..9 step *= 2 { y[0] = x[j] ^ c; y[1] = x[j] ^ c; }
         }
     }
     assign o = {7'd0, c};",
    false
);

counted_case!(
    counted_multiplied_inner_loop_keeps_a_first_iteration_read,
    "a write at the multiplied value read before the loop closes a loop",
    "var x: logic [9];
     var y: logic [9];
     var c: logic;
     always_comb {
         c = 0;
         for i in 0..2 {
             if i == 0 { c = y[2]; }
             for j in 1..9 step *= 2 { y[j] = x[j] ^ c; }
         }
     }
     assign o = {7'd0, c};",
    true
);

counted_case!(
    counted_enumerated_inner_loop_keeps_a_read_before_its_write,
    "an inner iteration that reads an element only a later one writes closes a loop",
    "var x: logic [4];
     var y: logic [4];
     var c: logic;
     always_comb {
         c = 0;
         for i in 0..2 {
             for j in 0..4 { if i == 0 { c = y[1 - (j >> 1)]; } y[j >> 1] = x[j] ^ c; }
         }
     }
     assign o = {7'd0, c};",
    true
);

counted_case!(
    counted_multiplied_inner_loop_positions_reach_a_later_iteration_read,
    "a later iteration reads the element a loop over multiplied values wrote at its value",
    "var x: logic [9];
     var y: logic [9];
     var c: logic;
     always_comb {
         c = 0;
         for i in 0..2 {
             if i == 1 { c = y[2]; }
             for j in 1..9 step *= 2 { y[j] = x[j] ^ c; }
         }
     }
     assign o = {7'd0, c};",
    false
);

counted_case!(
    counted_multiplied_inner_loop_positions_keep_their_order,
    "an iteration over multiplied values that reads a later one's element closes a loop",
    "var x: logic [9];
     var y: logic [9];
     var c: logic;
     always_comb {
         c = 0;
         for i in 0..1 {
             for j in 1..9 step *= 2 { c = y[9 - j]; y[j] = x[j] ^ c; }
         }
     }
     assign o = {7'd0, c};",
    true
);

counted_case!(
    counted_multiplied_inner_loop_positions_read_earlier_iterations,
    "an iteration over multiplied values reads what the earlier ones wrote",
    "var x: logic [9];
     var y: logic [9];
     var c: logic;
     always_comb {
         c = 0;
         for i in 0..1 {
             for j in 1..9 step *= 2 { c = y[j >> 1]; y[j] = x[j] ^ c; }
         }
     }
     assign o = {7'd0, c};",
    false
);

counted_case!(
    counted_breaking_inner_loop_write_before_its_break_reaches_a_later_iteration,
    "a write before an inner loop can break is what a later iteration reads",
    "var x: logic [4];
     var y: logic [4];
     var c: logic;
     always_comb {
         c = 0;
         for i in 0..2 {
             if i == 1 { c = y[0]; }
             for j in 0..4 { y[j] = x[j] ^ c; if x[j] { break; } }
         }
     }
     assign o = {7'd0, c};",
    false
);

counted_case!(
    counted_breaking_inner_loop_write_after_a_break_may_not_run,
    "a write an inner loop reaches only past a break leaves the earlier value",
    "var x: logic [4];
     var y: logic [4];
     var c: logic;
     always_comb {
         c = 0;
         for i in 0..2 {
             if i == 1 { c = y[1]; }
             for j in 0..4 { y[j] = x[j] ^ c; if x[j] { break; } }
         }
     }
     assign o = {7'd0, c};",
    true
);

counted_case!(
    counted_call_output_reaches_a_later_iteration_read,
    "a later iteration reads what a call wrote to its output",
    "var x: logic [4];
     var y: logic [4];
     var c: logic;
     function f (v: input logic, w: output logic) { w = v; }
     always_comb {
         c = 0;
         for i in 0..2 {
             if i == 1 { c = y[0]; }
             f(x[0] ^ c, y[0]);
         }
     }
     assign o = {7'd0, c};",
    false
);

counted_case!(
    counted_call_output_misses_a_first_iteration_read,
    "the first iteration reads the value from before a call writes its output",
    "var x: logic [4];
     var y: logic [4];
     var c: logic;
     function f (v: input logic, w: output logic) { w = v; }
     always_comb {
         c = 0;
         for i in 0..2 {
             if i == 0 { c = y[0]; }
             f(x[0] ^ c, y[0]);
         }
     }
     assign o = {7'd0, c};",
    true
);

counted_case!(
    counted_call_output_moving_with_the_iterator_reaches_the_next_iteration,
    "each iteration reads the element a call wrote on the one before",
    "var x: logic [4];
     var y: logic [4];
     var c: logic;
     function f (v: input logic, w: output logic) { w = v; }
     always_comb {
         c = 0;
         for i in 0..4 {
             if i >= 1 { c = y[i - 1]; }
             f(x[0] ^ c, y[i]);
         }
     }
     assign o = {7'd0, c};",
    false
);

counted_case!(
    counted_constant_bit_writes_reach_a_later_iteration_read,
    "a later iteration reads the bits an earlier one wrote beside others",
    "var x: logic [4];
     var e: logic<8>;
     var c: logic;
     always_comb {
         c = 0;
         for i in 0..2 {
             if i == 1 { c = e[7]; }
             e[7] = x[0] ^ c;
             e[6:0] = 0;
         }
     }
     assign o = {7'd0, c};",
    false
);

counted_case!(
    counted_member_writes_reach_a_later_whole_read,
    "a later iteration reads the whole struct an earlier one wrote member by member",
    "var x: logic [4];
     var r: pkg::Req;
     var c: logic;
     always_comb {
         c = 0;
         for i in 0..2 {
             if i == 1 { c = ^r; }
             r.go = x[0] ^ c;
             r.arm = 0;
             r.pad = 0;
         }
     }
     assign o = {7'd0, c};",
    false
);

counted_case!(
    counted_member_writes_miss_a_first_iteration_read,
    "the first iteration reads the struct from before the loop",
    "var x: logic [4];
     var r: pkg::Req;
     var c: logic;
     always_comb {
         c = 0;
         for i in 0..2 {
             if i == 0 { c = ^r; }
             r.go = x[0] ^ c;
             r.arm = 0;
             r.pad = 0;
         }
     }
     assign o = {7'd0, c};",
    true
);

counted_case!(
    counted_unwritten_bit_read_keeps_its_loop,
    "a bit the loop never writes is read from what follows it",
    "var x: logic [4];
     var e: logic<8>;
     var c: logic;
     always_comb {
         c = 0;
         for i in 0..2 {
             if i == 1 { c = ^e[7:6]; }
             e[7] = x[0];
             e[5:0] = 0;
         }
         e[6] = c;
     }
     assign o = {7'd0, c};",
    true
);

counted_case!(
    counted_runtime_inner_loop_leaves_other_reads_solved,
    "a runtime inner loop does not unsolve the reads of what it does not write",
    "var x: logic [4];
     var y: logic [4];
     var c: logic;
     always_comb {
         c = 0;
         for i in 0..2 {
             if i == 1 { c = y[0]; }
             y[0] = x[0] ^ c;
             for j in 0..a { y[1] = c; }
         }
     }
     assign o = {7'd0, c};",
    false
);

counted_case!(
    counted_runtime_inner_loop_writes_may_not_run,
    "a write in a runtime inner loop may leave the value from before the loop",
    "var x: logic [4];
     var y: logic [4];
     var c: logic;
     always_comb {
         c = 0;
         for i in 0..2 {
             if i == 1 { c = y[1]; }
             y[0] = x[0] ^ c;
             for j in 0..a { y[1] = c; }
         }
     }
     assign o = {7'd0, c};",
    true
);

counted_case!(
    counted_call_reading_other_variables_leaves_reads_solved,
    "a call that reads other variables but writes none keeps the reads solved",
    "var x: logic [4];
     var y: logic [4];
     var c: logic;
     function k () -> logic { return x[1]; }
     always_comb {
         c = 0;
         for i in 0..2 {
             if i == 1 { c = y[0]; }
             y[0] = k() ^ c;
         }
     }
     assign o = {7'd0, c};",
    false
);

counted_case!(
    counted_call_reading_the_written_storage_keeps_its_loop,
    "a call that reads the storage the loop writes closes the loop",
    "var y: logic [4];
     var c: logic;
     function k () -> logic { return y[0]; }
     always_comb {
         c = 0;
         for i in 0..2 {
             y[0] = k() ^ c;
         }
     }
     assign o = {7'd0, c};",
    true
);

counted_case!(
    counted_shift_through_a_scalar_is_feed_forward,
    "an element moves to the one before it through a scalar",
    "var x: logic [4];
     var y: logic [4];
     var c: logic;
     var e: logic;
     function g (v: input logic) -> logic { return !v; }
     function f (v: input logic, w: output logic) { w = v; }
     always_comb { y[3] = x[0]; for i in 0..3 { c = y[i + 1]; y[i] = c; } }
     assign o = {6'd0, c, e};",
    false
);

counted_case!(
    counted_shift_through_a_constant_element_is_feed_forward,
    "an element moves through a constant element of another array",
    "var x: logic [4];
     var y: logic [4];
     var c: logic;
     var e: logic;
     function g (v: input logic) -> logic { return !v; }
     function f (v: input logic, w: output logic) { w = v; }
     var t: logic [4];
     always_comb { y[3] = x[0]; t = '{0, 0, 0, 0}; for i in 0..3 { t[0] = y[i + 1]; y[i] = t[0]; } }
     assign o = {6'd0, c, e};",
    false
);

counted_case!(
    counted_shift_through_a_struct_member_is_feed_forward,
    "an element moves through a struct member",
    "var x: logic [4];
     var y: logic [4];
     var c: logic;
     var e: logic;
     function g (v: input logic) -> logic { return !v; }
     function f (v: input logic, w: output logic) { w = v; }
     var r: pkg::Req;
     always_comb { y[3] = x[0]; r = 0; for i in 0..3 { r.go = y[i + 1]; y[i] = r.go; } }
     assign o = {6'd0, c, e};",
    false
);

counted_case!(
    counted_shift_through_a_call_and_a_scalar_is_feed_forward,
    "an element moves through a call into a scalar",
    "var x: logic [4];
     var y: logic [4];
     var c: logic;
     var e: logic;
     function g (v: input logic) -> logic { return !v; }
     function f (v: input logic, w: output logic) { w = v; }
     always_comb { y[3] = x[0]; for i in 0..3 { c = g(y[i + 1]); y[i] = c; } }
     assign o = {6'd0, c, e};",
    false
);

counted_case!(
    counted_shift_through_a_call_output_is_feed_forward,
    "an element moves through the output of a call",
    "var x: logic [4];
     var y: logic [4];
     var c: logic;
     var e: logic;
     function g (v: input logic) -> logic { return !v; }
     function f (v: input logic, w: output logic) { w = v; }
     always_comb { y[3] = x[0]; for i in 0..3 { f(y[i + 1], c); y[i] = c; } }
     assign o = {6'd0, c, e};",
    false
);

counted_case!(
    counted_shift_through_a_conditional_scalar_is_feed_forward,
    "an element moves through a scalar assigned on a branch",
    "var x: logic [4];
     var y: logic [4];
     var c: logic;
     var e: logic;
     function g (v: input logic) -> logic { return !v; }
     function f (v: input logic, w: output logic) { w = v; }
     always_comb { y[3] = x[0]; for i in 0..3 { if a[0] { c = y[i + 1]; } else { c = 0; } y[i] = c; } }
     assign o = {6'd0, c, e};",
    false
);

counted_case!(
    counted_shift_through_scalars_in_turn_is_feed_forward,
    "an element moves through a chain of scalars, one reassigned",
    "var x: logic [4];
     var y: logic [4];
     var c: logic;
     var e: logic;
     function g (v: input logic) -> logic { return !v; }
     function f (v: input logic, w: output logic) { w = v; }
     always_comb { y[3] = x[0]; for i in 0..3 { c = y[i + 1]; c = c ^ x[i]; e = c; y[i] = e; } }
     assign o = {6'd0, c, e};",
    false
);

counted_case!(
    counted_shift_through_a_scalar_on_iterator_branches_is_feed_forward,
    "an element moves through a scalar each iterator branch assigns",
    "var x: logic [4];
     var y: logic [4];
     var c: logic;
     var e: logic;
     function g (v: input logic) -> logic { return !v; }
     function f (v: input logic, w: output logic) { w = v; }
     always_comb { y[3] = x[0]; for i in 0..3 { if i == 0 { c = y[1]; } else { c = y[i + 1]; } y[i] = c; } }
     assign o = {6'd0, c, e};",
    false
);

counted_case!(
    counted_shift_through_a_scalar_keeps_the_value_it_read,
    "a scalar keeps what it read though the element is written after",
    "var x: logic [4];
     var y: logic [4];
     var c: logic;
     var e: logic;
     function g (v: input logic) -> logic { return !v; }
     function f (v: input logic, w: output logic) { w = v; }
     always_comb { y[3] = x[0]; for i in 0..3 { c = y[i + 1]; y[i + 1] = 0; y[i] = c; } }
     assign o = {6'd0, c, e};",
    false
);

counted_case!(
    counted_bit_shift_through_a_scalar_is_feed_forward,
    "a bit moves to the one below it through a scalar",
    "var x: logic [4];
     var y: logic [4];
     var c: logic;
     var e: logic;
     function g (v: input logic) -> logic { return !v; }
     function f (v: input logic, w: output logic) { w = v; }
     var b8: logic<8>;
     always_comb { b8[7] = 0; for i in 0..7 { c = b8[i + 1]; b8[i] = c; } }
     assign p = b8;
     assign o = {6'd0, c, e};",
    false
);

counted_case!(
    counted_scalar_assigned_in_an_inner_loop_keeps_its_last_value,
    "what an inner loop leaves in a scalar is its last iteration's",
    "var x: logic [4];
     var y: logic [4];
     var c: logic;
     var e: logic;
     function g (v: input logic) -> logic { return !v; }
     function f (v: input logic, w: output logic) { w = v; }
     always_comb { y[3] = x[0]; for i in 0..3 { for j in 0..2 { c = y[i + 1] ^ x[j]; } y[i] = c; } }
     assign o = {6'd0, c, e};",
    false
);

counted_case!(
    counted_shift_through_a_scalar_keeps_a_closing_loop,
    "an element moved through a scalar into the first closes a loop",
    "var x: logic [4];
     var y: logic [4];
     var c: logic;
     var e: logic;
     function g (v: input logic) -> logic { return !v; }
     function f (v: input logic, w: output logic) { w = v; }
     always_comb { for i in 0..3 { c = y[i + 1]; y[i] = c; } y[3] = y[0]; }
     assign o = {6'd0, c, e};",
    true
);

counted_case!(
    counted_scalar_from_the_first_iteration_keeps_its_loop,
    "a scalar assigned on the first iteration only feeds later elements",
    "var x: logic [4];
     var y: logic [4];
     var c: logic;
     var e: logic;
     function g (v: input logic) -> logic { return !v; }
     function f (v: input logic, w: output logic) { w = v; }
     always_comb { y[3] = x[0]; c = 0; for i in 0..3 { if i == 0 { c = y[1]; } y[i] = c; } }
     assign o = {6'd0, c, e};",
    true
);

counted_case!(
    counted_scalar_read_before_it_is_assigned_keeps_its_loop,
    "a scalar read before its assignment takes the previous iteration's",
    "var x: logic [4];
     var y: logic [4];
     var c: logic;
     var e: logic;
     function g (v: input logic) -> logic { return !v; }
     function f (v: input logic, w: output logic) { w = v; }
     always_comb { y[3] = x[0]; c = 0; for i in 0..3 { y[i] = c; c = y[i + 1]; } }
     assign o = {6'd0, c, e};",
    true
);

counted_case!(
    counted_scalar_assigned_on_a_varying_branch_keeps_its_loop,
    "a scalar a data branch may skip keeps an earlier iteration's value",
    "var x: logic [4];
     var y: logic [4];
     var c: logic;
     var e: logic;
     function g (v: input logic) -> logic { return !v; }
     function f (v: input logic, w: output logic) { w = v; }
     always_comb { y[3] = x[0]; c = 0; for i in 0..3 { if a[i] { c = y[i + 1]; } y[i] = c; } }
     assign o = {6'd0, c, e};",
    true
);

counted_case!(
    counted_inner_iterator_read_reaches_its_own_elements,
    "an element read at the outer and inner iterators reaches only those",
    "var x: logic [4];
     var y: logic [4];
     var z: logic [4];
     var t: logic [4];
     var c: logic;
     always_comb { y[3] = x[0]; for i in 0..3 { for j in 1..2 { y[i] = y[i + j]; } } }
     assign o = {3'd0, c, y[0], y[1], y[2], y[3]};",
    false
);

counted_case!(
    counted_inner_iterator_window_is_feed_forward,
    "each element takes a window of another array the inner loop moves over",
    "var x: logic [4];
     var y: logic [4];
     var z: logic [4];
     var t: logic [4];
     var c: logic;
     always_comb { z[3] = x[0]; for i in 0..3 { y[i] = 0; for j in 1..2 { y[i] = y[i] ^ z[i + j]; } } }
     assign p = {4'd0, z[0], z[1], z[2], z[3]};
     assign o = {3'd0, c, y[0], y[1], y[2], y[3]};",
    false
);

counted_case!(
    counted_scalar_assigned_at_inner_positions_keeps_its_last_value,
    "a scalar an inner loop assigns from moving positions keeps the last",
    "var x: logic [4];
     var y: logic [4];
     var z: logic [4];
     var t: logic [4];
     var c: logic;
     always_comb { y[3] = x[0]; for i in 0..3 { for j in 0..2 { c = y[i + j]; } y[i] = c; } }
     assign o = {3'd0, c, y[0], y[1], y[2], y[3]};",
    false
);

counted_case!(
    counted_array_filled_by_an_inner_loop_is_feed_forward,
    "an element moves through an array an inner loop fills",
    "var x: logic [4];
     var y: logic [4];
     var z: logic [4];
     var t: logic [4];
     var c: logic;
     always_comb { y[3] = x[0]; t = '{0, 0, 0, 0}; for i in 0..3 { c = y[i + 1]; for j in 0..2 { t[j] = c; } y[i] = t[1]; } }
     assign o = {3'd0, c, y[0], y[1], y[2], y[3]};",
    false
);

counted_case!(
    counted_inner_window_into_the_first_element_keeps_its_loop,
    "a window over later elements that feeds the first closes a loop",
    "var x: logic [4];
     var y: logic [4];
     var z: logic [4];
     var t: logic [4];
     var c: logic;
     always_comb { for i in 0..3 { for j in 0..2 { y[i] = y[i + j]; } } y[3] = y[0]; }
     assign o = {3'd0, c, y[0], y[1], y[2], y[3]};",
    true
);

counted_case!(
    counted_array_filled_by_an_inner_loop_keeps_a_closing_loop,
    "an array an inner loop fills from later elements closes a loop",
    "var x: logic [4];
     var y: logic [4];
     var z: logic [4];
     var t: logic [4];
     var c: logic;
     always_comb { for i in 0..3 { for j in 0..2 { t[j] = y[i + 1]; } y[i] = t[1]; } y[3] = y[0]; t[2] = 0; t[3] = 0; }
     assign o = {3'd0, c, y[0], y[1], y[2], y[3]};",
    true
);

counted_case!(
    counted_array_element_the_inner_loop_skips_keeps_its_loop,
    "an element the inner loop does not write keeps an earlier iteration's value",
    "var x: logic [4];
     var y: logic [4];
     var z: logic [4];
     var t: logic [4];
     var c: logic;
     always_comb { t = '{0, 0, 0, 0}; for i in 0..3 { if i == 0 { t[1] = y[0]; } for j in 0..1 { t[j] = y[i + 1]; } y[i] = t[1]; } y[3] = x[0]; }
     assign o = {3'd0, c, y[0], y[1], y[2], y[3]};",
    true
);

counted_case!(
    counted_array_element_written_on_a_data_branch_keeps_its_loop,
    "an element a data branch may leave keeps an earlier iteration's value",
    "var x: logic [4];
     var y: logic [4];
     var z: logic [4];
     var t: logic [4];
     var c: logic;
     always_comb { t = '{0, 0, 0, 0}; for i in 0..3 { for j in 0..2 { if a[j] { t[j] = y[i + 1]; } } y[i] = t[1]; } y[3] = x[0]; }
     assign o = {3'd0, c, y[0], y[1], y[2], y[3]};",
    true
);

counted_case!(
    counted_array_filled_at_outer_and_inner_positions_is_feed_forward,
    "an array the inner loop fills at positions both iterators move is feed-forward",
    "var x: logic [4];
     var y: logic [4];
     var t: logic [4];
     always_comb { t = '{0, 0, 0, 0}; for i in 0..2 { for j in 0..2 { t[i + j] = y[i + 1]; } y[i] = t[1]; } y[2] = x[0]; y[3] = x[1]; }
     assign o = {4'd0, y[0], y[1], y[2], y[3]};",
    false
);

counted_case!(
    counted_array_filled_at_outer_and_inner_positions_keeps_a_closing_loop,
    "such an array that feeds back closes a loop",
    "var x: logic [4];
     var y: logic [4];
     var t: logic [4];
     always_comb { t = '{0, 0, 0, 0}; for i in 0..2 { for j in 0..2 { t[i + j] = y[i + 1]; } y[i] = t[1]; } y[2] = y[0]; y[3] = x[1]; }
     assign o = {4'd0, y[0], y[1], y[2], y[3]};",
    true
);

counted_case!(
    counted_array_element_no_inner_value_reaches_keeps_its_loop,
    "an element the inner loop reaches on only some iterations keeps an earlier value",
    "var x: logic [4];
     var y: logic [4];
     var t: logic [4];
     always_comb { t = '{0, 0, 0, 0}; for i in 0..3 { for j in 0..1 { t[i + j] = y[i + 1]; } y[i] = t[1]; } y[3] = x[1]; }
     assign o = {4'd0, y[0], y[1], y[2], y[3]};",
    true
);

counted_case!(
    counted_exclusive_arms_of_one_iteration_are_feed_forward,
    "arms of one branch on the same iteration exclude each other",
    "var t: logic [8];
     var y: logic [8];
     always_comb { for i in 0..3 { if a[6] { t[i] = y[2 * i]; } else { y[2 * i] = t[2]; } } }
     assign o = {2'd0, t[0], t[1], t[2], y[0], y[2], y[4]};",
    false
);

counted_case!(
    counted_arms_of_different_iterations_close_a_loop,
    "arms of one branch on different iterations both run",
    "var t: logic [8];
     var y: logic [8];
     always_comb { for i in 0..2 { if a[6] { t[0] = y[0]; } else { y[0] = t[0]; } } }
     assign o = {6'd0, t[0], y[0]};",
    true
);

counted_case!(
    counted_nested_overwrite_reads_only_its_last_instance,
    "an element each inner iteration overwrites holds only the last one's value",
    "var y: logic [8];
     always_comb { for i in 0..2 { for j in 0..2 { y[0] = y[j]; } } }
     assign o = {6'd0, y[0], y[1]};",
    false
);

counted_case!(
    counted_inner_loop_runs_whole_on_its_branch,
    "an inner loop on a branch writes on each of its iterations once the branch runs",
    "var x: logic [8];
     var y: logic [8];
     var c: logic;
     var e: logic;
     always_comb {
         for i in 0..2 { if a[2] { for j in rev 0..2 { y[i + j] = e; e = c; } c = y[2 * i]; } }
         c = x[3];
     }
     assign o = {c, e, y[0], y[1], y[2], y[3], 2'd0};",
    false
);

counted_case!(
    counted_value_carried_between_variables_reads_its_last_instance,
    "a value read from another variable the loop writes is that of its iteration",
    "var t: logic [8];
     var c: logic;
     always_comb { for i in 0..2 { for j in 0..2 { t[0] = c; c = a[3]; } } c = t[0]; }
     assign o = {7'd0, t[0]};",
    false
);
