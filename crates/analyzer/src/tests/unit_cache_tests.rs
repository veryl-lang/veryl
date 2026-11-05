use super::*;
use crate::conv::unit_cache;

/// Converts `code` as usual, then records every unit and converts again
/// replaying them, and checks that the replay reports the same diagnostics.
#[track_caller]
fn assert_replay_reproduces(code: &str) -> Vec<AnalyzerError> {
    let (errors, stubs) = replay(&[(code, true)]);
    assert!(stubs > 0, "nothing was replayed");
    errors
}

/// Like [`assert_replay_reproduces`] over several files, replaying the
/// components of those marked reusable. As in an incremental build, only the
/// others are converted, unless all are reusable; the records come from
/// converting them the same way.
#[track_caller]
fn assert_replay_reproduces_files(files: &[(&str, bool)]) -> Vec<AnalyzerError> {
    let (errors, stubs) = replay(files);
    assert!(stubs > 0, "nothing was replayed");
    errors
}

/// The diagnostics, checked to be the same with and without the replay, and
/// how many components the replay converted from the header alone.
#[track_caller]
fn replay(files: &[(&str, bool)]) -> (Vec<AnalyzerError>, usize) {
    symbol_table::clear();
    attribute_table::clear();
    doc_comment_table::clear();

    let metadata = Metadata::create_default("prj").unwrap();
    let analyzer = Analyzer::new(&metadata);
    let parsers: Vec<_> = files
        .iter()
        .enumerate()
        .map(|(i, (code, _))| Parser::parse(code, &format!("test_{i}.veryl")).unwrap())
        .collect();
    for parser in &parsers {
        analyzer.analyze_pass1("prj", &parser.veryl);
    }
    Analyzer::analyze_post_pass1();

    let all_reusable = files.iter().all(|(_, reusable)| *reusable);
    let convert = || loop {
        crate::resolved_type_table::clear();
        crate::msb_table::clear();
        unit_cache::begin_pass();
        let mut context = Context::default();
        if unit_cache::is_active() {
            context.instance_history.track_keys();
        }
        let mut ir = Ir::default();
        let mut errors = vec![];
        for (parser, (_, reusable)) in parsers.iter().zip(files) {
            if all_reusable || !reusable {
                errors.append(&mut analyzer.analyze_pass2(
                    &parser.veryl,
                    &mut context,
                    Some(&mut ir),
                ));
            }
        }
        errors.append(&mut Analyzer::analyze_post_pass2(&ir));
        if !unit_cache::force_requested() {
            break errors;
        }
    };

    unit_cache::clear();
    let expected = convert();
    unit_cache::set_recording(true);
    convert();
    unit_cache::set_recording(false);
    let reusable = files
        .iter()
        .enumerate()
        .filter(|(_, (_, reusable))| *reusable)
        .map(|(i, _)| {
            veryl_parser::resource_table::insert_path(std::path::Path::new(&format!(
                "test_{i}.veryl"
            )))
        })
        .collect();
    unit_cache::set_reusable_files(Some(reusable));
    let replayed = convert();
    let stubs = unit_cache::stub_count();
    unit_cache::clear();

    assert_eq!(replayed, expected);
    (expected, stubs)
}

#[test]
fn replay_keeps_instance_notes_and_merges() {
    let errors = assert_replay_reproduces(
        r#"
        module Leaf #(param W: u32 = 1) (o: output logic<W>) {
            var a: logic<W>;
            assign a = 1;
            assign a = 2;
            assign o = a;
        }
        module Mid #(param W: u32 = 1) (o: output logic<W>) {
            inst u: Leaf #(W) (o);
        }
        module Top (o1: output logic<2>, o2: output logic<3>, o3: output logic) {
            inst a: Mid #(W: 2) (o: o1);
            inst b: Mid #(W: 3) (o: o2);
            inst c: Leaf (o: o3);
        }
        "#,
    );
    assert!(
        errors
            .iter()
            .any(|x| matches!(x, AnalyzerError::MultipleAssignment { .. }))
    );
}

#[test]
fn replay_reports_recursion() {
    let errors = assert_replay_reproduces(
        r#"
        module ModuleA { inst u: ModuleB; }
        module ModuleB { inst u: ModuleA; }
        module ModuleC { inst u: ModuleA; inst v: ModuleD; }
        module ModuleD {}
        "#,
    );
    assert!(
        errors
            .iter()
            .any(|x| matches!(x, AnalyzerError::InfiniteRecursion { .. }))
    );
}

#[test]
fn replay_keeps_combinational_loops() {
    let errors = assert_replay_reproduces(
        r#"
        module Pass (i: input logic, o: output logic) { assign o = i; }
        module Inner { var y: logic; assign y = ~y; }
        module Outer { inst u: Inner; }
        module Top {
            var x: logic;
            inst p: Pass (i: x, o: x);
            inst q: Outer;
        }
        module Again { inst u: Outer; }
        "#,
    );
    assert!(
        errors
            .iter()
            .any(|x| matches!(x, AnalyzerError::CombinationalLoop { .. }))
    );
}

#[test]
fn replay_tells_same_shaped_type_parameters_apart() {
    let errors = assert_replay_reproduces(
        r#"
        package Pkg {
            enum E1: logic<2> { A, B }
            enum E2: logic<2> { C, D }
        }
        module Pass #(param T: type = logic) (i: input T, o: output T) {
            assign o = i;
        }
        module Top (i: input Pkg::E1, o: output Pkg::E1) {
            var y: Pkg::E2;
            inst a: Pass #(T: Pkg::E1) (i, o);
            inst b: Pass #(T: Pkg::E2) (i: y, o: y);
        }
        "#,
    );
    assert!(
        errors
            .iter()
            .any(|x| matches!(x, AnalyzerError::CombinationalLoop { .. }))
    );
}

#[test]
fn replay_summarizes_a_child_first_reached_below_a_replayed_parent() {
    // `A` is analyzed first and reaches `B #(W: 2)` through its record, which
    // `C` then needs a summary of to find its loop. Tracing the loop into `B`
    // converts it again, so nothing stays replayed.
    let (errors, _) = replay(&[
        (
            "module A (i: input logic, o: output logic) { inst b: B #(W: 2) (i, o); }",
            true,
        ),
        (
            "module B #(param W: u32 = 1) (i: input logic, o: output logic) { assign o = i; }",
            true,
        ),
        (
            r#"
            module C (o: output logic, p: output logic) {
                var x: logic;
                inst a: A (i: 1'b0, o: p);
                inst b: B #(W: 2) (i: x, o: x);
                assign o = x;
            }
            "#,
            false,
        ),
    ]);
    assert!(
        errors
            .iter()
            .any(|x| matches!(x, AnalyzerError::CombinationalLoop { .. }))
    );
}

#[test]
fn replay_converts_modules_driving_interfaces() {
    let errors = assert_replay_reproduces(
        r#"
        interface Bus { var data: logic<8>; }
        module Child (a: interface, b: interface) { assign a.data = 8'h12; assign b.data = 8'h34; }
        module Pass (a: interface, b: interface) { inst u: Child (a, b); }
        module Top (data: output logic<8>) {
            inst bus: Bus;
            inst u: Pass (a: bus, b: bus);
            assign data = bus.data;
        }
        "#,
    );
    assert!(
        errors
            .iter()
            .any(|x| matches!(x, AnalyzerError::MultipleAssignment { .. }))
    );
}

#[test]
fn replay_keeps_bodies_read_by_tests() {
    let errors = assert_replay_reproduces_files(&[
        (
            r#"
            module Sub (i: input logic) {
                #[allow(unused_variable)]
                var r: logic;
                assign r = i;
            }
            module Top (i: input logic) {
                inst u_sub: Sub (i);
            }
            "#,
            true,
        ),
        (
            r#"
            module Other (i: input logic) {
                inst t: Top (i);
            }
            #[test(hier_test)]
            module hier_test {
                var i: logic;
                inst dut: Top (i);
                initial {
                    i = 1;
                    $assert(dut.u_sub.r == 1);
                    $assert(dut.u_sub.missing == 1);
                    $finish();
                }
            }
            "#,
            false,
        ),
    ]);
    assert!(
        errors
            .iter()
            .any(|x| matches!(x, AnalyzerError::UnknownMember { .. })),
        "{errors:?}"
    );
}

#[test]
fn replay_keeps_inferred_type_conflicts() {
    // The signatures in conflict are converted again instead, so nothing
    // stays replayed.
    let (errors, _) = replay(&[
        (
            r#"
            package Types {
                type Narrow = logic<8>;
                type Wide = logic<16>;
            }
            module Gen::<T: type> {
                var a: T;
                assign a = 0;
                var _v;
                assign _v = a;
            }
            module Mid {
                inst u: Gen::<Types::Narrow>;
            }
            "#,
            true,
        ),
        (
            r#"
            module Top {
                inst m: Mid;
                inst u: Gen::<Types::Wide>;
            }
            "#,
            false,
        ),
    ]);
    assert!(
        errors
            .iter()
            .any(|x| matches!(x, AnalyzerError::TypeInferenceConflict { .. })),
        "{errors:?}"
    );
}

#[test]
fn replay_keeps_bodies_below_an_earlier_parent() {
    // `M` keeps the header of `C` it got before the test converts `C` whole.
    let (errors, _) = replay(&[
        (
            r#"
            module C (i: input logic) {
                #[allow(unused_variable)]
                var r: logic;
                assign r = i;
            }
            "#,
            true,
        ),
        (
            r#"
            module M (i: input logic) {
                inst u_c: C (i);
            }
            #[test(hier_test)]
            module hier_test {
                var i: logic;
                inst c: C (i);
                inst m: M (i);
                initial {
                    i = 1;
                    $assert(m.u_c.r == 1);
                    $finish();
                }
            }
            "#,
            false,
        ),
    ]);
    assert!(
        !errors
            .iter()
            .any(|x| matches!(x, AnalyzerError::UnknownMember { .. })),
        "{errors:?}"
    );
}
