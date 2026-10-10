use super::*;
use crate::comb_loop_detect::{analysis_size, reset_analysis_size};

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
fn crossing_writes_complete_and_preserve_parent_cycles() {
    // Row writes and dynamically indexed column writes cut both axes of the
    // same storage. Writes are regions of one storage node, so neither the
    // child nor its parent is limited by a row-by-column partition.
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
        let errors = analyze(&code);
        assert!(comb_loop_analysis_is_complete(&code));
        assert!(
            errors.iter().all(|error| match error {
                AnalyzerError::CombinationalLoop { identifier, .. }
                | AnalyzerError::UnassignVariable { identifier, .. } => identifier == "independent",
                _ => false,
            }),
            "{errors:#?}"
        );
        assert_eq!(errors.iter().filter(|error| matches!(error,
            AnalyzerError::CombinationalLoop { identifier, .. } if identifier == "independent"
        )).count(), 1, "{errors:#?}");
    }
}

#[test]
fn crossing_writes_do_not_invent_feedback_or_leak_between_modules() {
    let code = crossing_writes(64);
    assert!(analyze(&code).is_empty());
    assert!(comb_loop_analysis_is_complete(&code));
    let simple = "module Top(i: input logic<1000003>, o: output logic<1000003>) { assign o = i; }";
    assert!(analyze(simple).is_empty());
    assert!(comb_loop_analysis_is_complete(simple));
}

#[test]
fn optimized_read_views_and_rotations_are_complete() {
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
}

#[test]
fn generated_write_matrix_is_complete_without_a_storage_partition() {
    // Two linear groups of writes split both axes of the same storage. The
    // former atom partition needed 1088 * 1088 atoms here; regional writes
    // keep one storage region and a linear write history.
    let code = crossing_writes(1088);
    reset_analysis_size();
    let errors = analyze(&code);
    assert!(errors.is_empty(), "{errors:#?}");
    let (atoms, nodes, edges) = analysis_size();
    assert!(atoms <= 4, "{atoms} atoms");
    assert!(
        nodes <= 8 * 1088 && edges <= 8 * 1088,
        "{nodes} nodes, {edges} edges"
    );
    assert!(comb_loop_analysis_is_complete(&code));
}

#[test]
fn partition_limit_default_bounds_byte_enabled_memory_next_state() {
    // 32,768 512-bit words are a 2 MiB memory. A common per-element default
    // copy followed by byte-enabled writes once created a word-by-byte
    // partition. Writes now record their regions instead, so the analysis
    // stays linear in the writes.
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
        // Each variable is one storage node, so neither the rows nor the
        // byte lanes cut the storage. The per-row copy writes one region per
        // enumerated row; the whole-array copy writes one.
        let (atoms, nodes, edges) = analysis_size();
        assert_eq!(atoms, 5, "whole_copy={whole_copy}");
        let rows = if whole_copy { 0 } else { 32768 };
        assert!(
            nodes <= rows + 512 && edges <= 2 * rows + 512,
            "whole_copy={whole_copy}: {nodes} nodes, {edges} edges"
        );
        assert!(comb_loop_analysis_is_complete(&code));
    }
}

#[test]
fn scattered_element_writes_resolve_in_linear_size() {
    // Writes in bit-reversed order leave a fragment between every pair of
    // written elements while the final value is resolved. Indexed fragments
    // keep that resolution, and the exported graph, linear in the writes.
    const COUNT: usize = 8192;
    let bits = COUNT.trailing_zeros();
    let writes = (0..COUNT)
        .map(|k| k.reverse_bits() >> (usize::BITS - bits))
        .map(|k| format!("o[{k}] = i;"))
        .collect::<String>();
    let code = format!(
        "module Scattered (i: input logic<2>, o: output logic<2> [{COUNT}]) {{
            always_comb {{ {writes} }}
        }}"
    );
    reset_analysis_size();
    assert!(analyze(&code).is_empty());
    let (_, nodes, edges) = analysis_size();
    assert!(
        nodes <= 3 * COUNT && edges <= 4 * COUNT,
        "{nodes} nodes, {edges} edges"
    );
    assert!(comb_loop_analysis_is_complete(&code));
}
