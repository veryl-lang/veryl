use super::*;
use crate::comb_loop_detect::{
    analysis_size, reset_analysis_size, with_partition_extra_atom_limit,
};

fn crossing_writes(count: usize) -> String {
    let mut writes = String::new();
    for row in 0..count {
        writes += &format!("o[{row}] = data[{row}];\n");
    }
    for bit in 0..count {
        writes += &format!("o[index][{bit}] = 1'b0;\n");
    }
    format!(
        "module Fragmented(index: input u32, data: input logic<{count}>[{count}],
                           o: output logic<{count}>[{count}]) {{
            always_comb {{ o = data; {writes} }}
        }}"
    )
}

#[test]
fn partition_limit_marks_residual_write_expansion_incomplete_and_preserves_parent_cycles() {
    for count in [8, 64] {
        let code = format!(
            "{}
             module Top(index: input u32, data: input logic<{count}>[{count}],
                        o: output logic<{count}>[{count}], independent: output logic) {{
                inst child: Fragmented(index, data, o);
                assign independent = ~independent;
             }}",
            crossing_writes(count)
        );
        with_partition_extra_atom_limit(0, || {
            let errors = analyze(&code);
            assert_eq!(comb_loop_analysis_is_complete(&code), count == 8);
            assert!(
                errors.iter().all(|error| match error {
                    AnalyzerError::CombinationalLoop { identifier, .. }
                    | AnalyzerError::UnassignVariable { identifier, .. } =>
                        identifier == "independent",
                    _ => false,
                }),
                "{errors:#?}"
            );
            assert_eq!(errors.iter().filter(|error| matches!(error,
                AnalyzerError::CombinationalLoop { identifier, .. } if identifier == "independent"
            )).count(), 1, "{errors:#?}");
        });
    }
}

#[test]
fn partition_limit_does_not_invent_feedback_or_leak_between_modules() {
    let code = crossing_writes(64);
    with_partition_extra_atom_limit(0, || {
        let errors = analyze(&code);
        assert!(errors.is_empty(), "{errors:#?}");
        assert!(!comb_loop_analysis_is_complete(&code));
        let simple =
            "module Top(i: input logic<1000003>, o: output logic<1000003>) { assign o = i; }";
        assert!(analyze(simple).is_empty());
        assert!(comb_loop_analysis_is_complete(simple));
    });
    // The default allowance admits this small write partition in full.
    assert!(analyze(&code).is_empty());
    assert!(comb_loop_analysis_is_complete(&code));
}

#[test]
fn partition_limit_keeps_optimized_read_views_and_rotations_complete() {
    with_partition_extra_atom_limit(0, || {
        let count = 256;
        let reads = (0..count)
            .map(|bit| format!("assign o[{bit}] = mem[{bit}][{bit}] ^ mem[index][{bit}];"))
            .collect::<Vec<_>>()
            .join("\n");
        let code = format!(
            "module Top(index: input u32, mem: input logic<{count}>[{count}], o: output logic<{count}>) {{ {reads} }}"
        );
        assert!(analyze(&code).is_empty());
        assert!(comb_loop_analysis_is_complete(&code));

        let code = "module Top(o: output logic<1000003>) { var a: logic<1000003>; assign a = {o[999999:0], o[1000002:1000000]}; assign o = a; }";
        let errors = analyze(code);
        assert!(
            matches!(errors.as_slice(), [AnalyzerError::CombinationalLoop { .. }]),
            "{errors:#?}"
        );
        assert!(comb_loop_analysis_is_complete(code));
    });
}

#[test]
fn partition_limit_default_skips_a_legal_generated_write_matrix_without_diagnostics() {
    // Two linear groups of writes split both axes of the same storage. The
    // resulting 1088 * 1088 atoms exceed the default allowance before SSA
    // construction. Declaring the array alone does not cause this expansion.
    let code = crossing_writes(1088);
    reset_analysis_size();
    let errors = analyze(&code);
    assert!(errors.is_empty(), "{errors:#?}");
    assert_eq!(
        analysis_size(),
        (0, 0, 0),
        "the unfinished partition must not reach graph construction"
    );
    assert!(!comb_loop_analysis_is_complete(&code));

    let whole =
        "module Top(i: input logic<1088>[1088], o: output logic<1088>[1088]) { assign o = i; }";
    with_partition_extra_atom_limit(0, || {
        assert!(analyze(whole).is_empty());
        assert!(comb_loop_analysis_is_complete(whole));
    });
}

#[test]
fn partition_limit_default_bounds_byte_enabled_memory_next_state() {
    // 32,768 512-bit words are a 2 MiB memory. A common per-element default
    // copy followed by byte-enabled writes still creates a word-by-byte
    // partition. The equivalent whole-array copy avoids the array cuts.
    for whole_copy in [false, true] {
        let initialization = if whole_copy {
            "next_data = data;"
        } else {
            "for row in 0..32768 { next_data[row] = data[row]; }"
        };
        let code = format!(
            r#"module ByteWrite (
    data: input logic<512>[32768],
    address: input logic<15>,
    write_data: input logic<512>,
    byte_enable: input logic<64>,
    next_data: output logic<512>[32768],
) {{
    always_comb {{
        {initialization}
        for lane in 0..64 {{
            if byte_enable[lane] {{
                next_data[address][lane * 8 +: 8] = write_data[lane * 8 +: 8];
            }}
        }}
    }}
}}
"#
        );
        reset_analysis_size();
        let errors = analyze(&code);
        assert!(errors.is_empty(), "whole_copy={whole_copy}: {errors:#?}");
        if whole_copy {
            assert_eq!(analysis_size().0, 67);
        } else {
            assert_eq!(analysis_size(), (0, 0, 0));
        }
        assert_eq!(comb_loop_analysis_is_complete(&code), whole_copy);
    }
}
