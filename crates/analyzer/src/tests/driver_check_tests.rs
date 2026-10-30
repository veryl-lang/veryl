use super::*;
use crate::ir::Extent;
use crate::ir::span::PackedSpan;

fn multiple_assignments(errors: &[AnalyzerError]) -> Vec<&AnalyzerError> {
    errors
        .iter()
        .filter(|e| matches!(e, AnalyzerError::MultipleAssignment { .. }))
        .collect()
}

const GENERIC_GENERATE_IF: &str = r#"
module sub::<ENABLE: bbool> (
  i_a: input  logic,
  o_a: output logic,
  o_b: output logic,
) {
  var a: logic;
  var b: logic;

  always_comb {
    a = i_a;
  }

  if ENABLE :g {
    always_comb {
      b = i_a;
    }
  } else {
    always_comb {
      a = '0;
    }
  }

  always_comb {
    o_a = a;
    o_b = b;
  }
}

module top (
  i_a: input  logic,
  o_a: output logic,
  o_b: output logic,
) {
  inst u_sub: sub::<VALUE> (
    i_a: i_a,
    o_a: o_a,
    o_b: o_b,
  );
}
"#;

#[test]
fn generic_generate_branch_driver_is_checked_per_specialization() {
    // Issue #3570: the second driver of `a` exists only in `sub::<false>`.
    let errors = analyze(&GENERIC_GENERATE_IF.replace("VALUE", "false"));
    assert_eq!(multiple_assignments(&errors).len(), 1, "{errors:?}");

    // `b` stays undriven in `sub::<false>`; that is a separate check.
    let errors = analyze(&GENERIC_GENERATE_IF.replace("VALUE", "true"));
    assert!(multiple_assignments(&errors).is_empty(), "{errors:?}");
}

#[test]
fn specializations_with_the_same_conflict_report_once() {
    let code = r#"
    module sub::<W: u32> (
      o: output logic<W>,
    ) {
      always_comb {
        o = '0;
      }
      always_comb {
        o = '1;
      }
    }

    module top {
      inst u0: sub::<1> (o: _);
      inst u1: sub::<2> (o: _);
      inst u2: sub::<2> (o: _);
    }
    "#;
    let errors = analyze(code);
    assert_eq!(multiple_assignments(&errors).len(), 1, "{errors:?}");
}

#[test]
fn variables_with_the_same_name_in_different_modules_are_separate() {
    let code = r#"
    module ModuleA {
      var a: logic;
      always_comb {
        a = 0;
      }
      always_comb {
        a = 1;
      }
    }

    module ModuleB {
      var a: logic;
      always_comb {
        a = 0;
      }
      always_comb {
        a = 1;
      }
    }
    "#;
    let errors = analyze(code);
    assert_eq!(multiple_assignments(&errors).len(), 2, "{errors:?}");
}

#[test]
fn two_output_ports_of_one_instance_driving_one_variable() {
    let code = r#"
    module Child (
      o0: output logic,
      o1: output logic,
    ) {
      assign o0 = 0;
      assign o1 = 1;
    }

    module Top {
      var a: logic;
      inst u: Child (
        o0: a,
        o1: a,
      );
    }
    "#;
    let errors = analyze(code);
    assert_eq!(multiple_assignments(&errors).len(), 1, "{errors:?}");

    let code = r#"
    module Child (
      o0: output logic,
      o1: output logic,
    ) {
      assign o0 = 0;
      assign o1 = 1;
    }

    module Top {
      var a: logic<2>;
      inst u: Child (
        o0: a[0],
        o1: a[1],
      );
    }
    "#;
    let errors = analyze(code);
    assert!(multiple_assignments(&errors).is_empty(), "{errors:?}");
}

#[test]
fn repeated_function_output_in_one_process_is_legal() {
    let code = r#"
    module ModuleA (
        clk: input clock,
    ) {
        var a: logic;

        function FuncA(o: output logic) {
            o = 0;
        }

        always_ff {
            FuncA(a);
            FuncA(a);
        }
    }
    "#;
    let errors = analyze(code);
    assert!(multiple_assignments(&errors).is_empty(), "{errors:?}");
}

#[test]
fn arrays_over_the_evaluation_limit_are_checked() {
    let code = r#"
    module ModuleA {
      var a: logic[1024];
      always_comb {
        a[1000] = 0;
      }
      always_comb {
        a[1000] = 1;
      }
      always_comb {
        a[3] = 1;
      }
    }
    "#;
    let errors = analyze(code);
    assert_eq!(multiple_assignments(&errors).len(), 1, "{errors:?}");
}

#[test]
fn reported_sites_are_only_the_conflicting_writes() {
    let code = r#"
    module ModuleA {
      var a: logic<4>;
      always_comb {
        a[0] = 0;
        a[3:2] = 0;
      }
      always_comb {
        a[1] = 0;
        a[2] = 1;
      }
    }
    "#;
    let errors = analyze(code);
    let found = multiple_assignments(&errors);
    assert_eq!(found.len(), 1, "{errors:?}");
    let AnalyzerError::MultipleAssignment {
        error_locations, ..
    } = found[0]
    else {
        unreachable!()
    };
    assert_eq!(error_locations.len(), 2, "{errors:?}");
}

#[test]
fn procedural_loop_writes_cover_their_static_prefix() {
    // The emitted SV keeps the loop, so `a[i]` is not a static index: both
    // processes write all of `a`.
    let code = r#"
    module ModuleA {
      var a: logic[8];
      always_comb {
        for i in 0..4 {
          a[i] = 0;
        }
      }
      always_comb {
        for i in 4..8 {
          a[i] = 1;
        }
      }
    }
    "#;
    let errors = analyze(code);
    assert_eq!(multiple_assignments(&errors).len(), 1, "{errors:?}");

    // Within one process the writes never conflict.
    let code = r#"
    module ModuleA {
      var a: logic[8];
      always_comb {
        for i in 0..4 {
          a[i] = 0;
        }
        for i in 0..8 {
          a[i] = 1;
        }
      }
    }
    "#;
    let errors = analyze(code);
    assert!(multiple_assignments(&errors).is_empty(), "{errors:?}");
}

#[test]
fn function_output_inside_expression_drives() {
    let code = r#"
    module ModuleA (
        r: output logic,
    ) {
        var a: logic;

        function FuncA(o: output logic) -> logic {
            o = 0;
            return 0;
        }

        always_comb {
            r = FuncA(a);
        }
        always_comb {
            a = 1;
        }
    }
    "#;
    let errors = analyze(code);
    assert_eq!(multiple_assignments(&errors).len(), 1, "{errors:?}");
}

#[test]
fn readmemh_drives_its_destination() {
    let code = r#"
    module ModuleA {
        ATTR
        var rom: logic<32> [4];

        initial {
            $readmemh("rom.hex", rom);
        }
        always_comb {
            rom[0] = 0;
        }
    }
    "#;
    let errors = analyze(&code.replace("ATTR", ""));
    assert_eq!(multiple_assignments(&errors).len(), 1, "{errors:?}");

    let errors = analyze(&code.replace("ATTR", "#[allow(initial_assign)]"));
    assert!(multiple_assignments(&errors).is_empty(), "{errors:?}");
}

#[test]
fn specializations_with_different_first_sites_are_merged() {
    // `sub::<0>` conflicts on the first two assignments, `sub::<1>` on the
    // last two: one diagnostic for `o` with all three sites.
    let code = r#"
    module sub::<P: u32> (
        o: output logic<2>,
    ) {
        assign o[0] = 0;
        assign o[P] = 1;
        assign o[1] = 0;
    }

    module top (
        a: output logic<2>,
        b: output logic<2>,
    ) {
        inst u0: sub::<0> (o: a);
        inst u1: sub::<1> (o: b);
    }
    "#;
    let errors = analyze(code);
    let found = multiple_assignments(&errors);
    assert_eq!(found.len(), 1, "{errors:?}");
    let AnalyzerError::MultipleAssignment {
        error_locations, ..
    } = found[0]
    else {
        unreachable!()
    };
    assert_eq!(error_locations.len(), 3, "{errors:?}");
}

#[test]
fn loop_iterations_are_coalesced_under_their_claim() {
    let code = r#"
    module ModuleA (
        o: output logic<4096>,
    ) {
        always_comb {
            for i in 0..4096 {
                o[i] = 1'b0;
            }
        }
        OTHER
    }
    "#;

    symbol_table::clear();
    attribute_table::clear();
    doc_comment_table::clear();
    let metadata = Metadata::create_default("prj").unwrap();
    let parser = Parser::parse(&code.replace("OTHER", ""), &"").unwrap();
    let analyzer = Analyzer::new(&metadata);
    let mut context = Context::default();
    let mut ir = Ir::default();
    analyzer.analyze_pass1("prj", &parser.veryl);
    Analyzer::analyze_post_pass1();
    analyzer.analyze_pass2(&parser.veryl, &mut context, Some(&mut ir));
    let module = ir.module_postorder()[0];
    let extents: Vec<_> = module.write_summary.processes[0]
        .writes
        .iter()
        .map(|x| (x.extent, x.packed))
        .collect();
    assert_eq!(
        extents,
        vec![
            (Extent::Claimed, PackedSpan::whole(4096).unwrap()),
            (Extent::Iteration, PackedSpan::whole(4096).unwrap()),
        ]
    );

    let errors = analyze(&code.replace("OTHER", "assign o[0] = 1'b1;"));
    assert_eq!(multiple_assignments(&errors).len(), 1, "{errors:?}");
}

#[test]
fn interleaved_loop_writes_are_coalesced() {
    let code = r#"
    module ModuleA (
        o: output logic<2>,
        p: output logic<64>,
    ) {
        always_comb {
            for i in 0..64 {
                for j in 0..64 {
                    o[0] = 1'b0;
                    o[1] = 1'b1;
                    p[j] = 1'b0;
                }
            }
        }
    }
    "#;

    symbol_table::clear();
    attribute_table::clear();
    doc_comment_table::clear();
    let metadata = Metadata::create_default("prj").unwrap();
    let parser = Parser::parse(code, &"").unwrap();
    let analyzer = Analyzer::new(&metadata);
    let mut context = Context::default();
    let mut ir = Ir::default();
    analyzer.analyze_pass1("prj", &parser.veryl);
    Analyzer::analyze_post_pass1();
    analyzer.analyze_pass2(&parser.veryl, &mut context, Some(&mut ir));
    let module = ir.module_postorder()[0];
    let iterations = module.write_summary.processes[0]
        .writes
        .iter()
        .filter(|x| x.extent == Extent::Iteration)
        .count();
    // One region for each assignment, not for each iteration.
    assert_eq!(iterations, 3);
}
