use super::*;

fn invalid_select_note(errors: &[AnalyzerError]) -> Option<(String, usize, String)> {
    note_of(errors, |x| matches!(x, AnalyzerError::InvalidSelect { .. }))
}

fn note_of(
    errors: &[AnalyzerError],
    target: fn(&AnalyzerError) -> bool,
) -> Option<(String, usize, String)> {
    use miette::Diagnostic;

    let error = errors
        .iter()
        .find(|x| target(x))
        .unwrap_or_else(|| panic!("{errors:?}"));
    let note = error.instance()?;
    let contents = note
        .source_code()
        .unwrap()
        .read_span(&note.span(), 0, 0)
        .unwrap();
    let text = String::from_utf8(contents.data().to_vec()).unwrap();
    Some((note.path().to_string(), contents.line(), text))
}

fn site_of(code: &str, pat: &str) -> (usize, String) {
    let offset = code.find(pat).unwrap();
    let line = code[..offset].matches('\n').count();
    (line, pat.split(':').next().unwrap().to_string())
}

fn note(path: &str, code: &str, pat: &str) -> Option<(String, usize, String)> {
    let (line, name) = site_of(code, pat);
    Some((path.to_string(), line, name))
}

#[test]
fn error_in_overridden_instance_names_the_instantiation() {
    let code = r#"
    module Leaf #(param W: u32 = 1) (i: input logic<8>, o: output logic) {
        assign o = i[W + 5];
    }
    module Mid #(param N: u32 = 1) (i: input logic<8>, o: output logic) {
        inst l: Leaf #(W: N * 2) (i, o);
    }
    module Top (i: input logic<8>, o: output logic) {
        inst m: Mid #(N: 3) (i, o);
    }
    "#;

    let errors = analyze(code);
    assert_eq!(
        invalid_select_note(&errors),
        note("m: Mid #(N: 3) -> l: Leaf #(W: 6)", code, "m: Mid")
    );
}

#[test]
fn instantiation_note_stops_at_a_default_instance() {
    // `w` is at its defaults, so only the override inside `Wrap` decides `W`.
    let code = r#"
    module Leaf #(param W: u32 = 1) (i: input logic<8>, o: output logic) {
        assign o = i[W + 5];
    }
    module Wrap (i: input logic<8>, o: output logic) {
        inst l: Leaf #(W: 9) (i, o);
    }
    module Top (i: input logic<8>, o: output logic) {
        inst w: Wrap (i, o);
    }
    "#;

    let errors = analyze(code);
    assert_eq!(
        invalid_select_note(&errors),
        note("l: Leaf #(W: 9)", code, "l: Leaf")
    );
}

#[test]
fn error_at_default_parameters_has_no_instantiation_note() {
    let code = r#"
    module Leaf #(param W: u32 = 9) (i: input logic<8>, o: output logic) {
        assign o = i[W + 5];
    }
    module Top (i: input logic<8>, o: output logic) {
        inst l: Leaf (i, o);
    }
    "#;

    let errors = analyze(code);
    assert_eq!(invalid_select_note(&errors), None);
}

#[test]
fn same_error_through_another_instance_is_reported_once() {
    // One report, kept with the first instantiation.
    let code = r#"
    module Leaf #(param W: u32 = 1, param D: u32 = 0) (i: input logic<8>, o: output logic) {
        assign o = i[W + 5];
    }
    module Top (i: input logic<8>, o: output logic, p: output logic) {
        inst a: Leaf #(W: 9, D: 1) (i, o);
        inst b: Leaf #(W: 9, D: 2) (i, o: p);
    }
    "#;

    let errors = analyze(code);
    let selects: Vec<_> = errors
        .iter()
        .filter(|x| matches!(x, AnalyzerError::InvalidSelect { .. }))
        .collect();
    assert_eq!(selects.len(), 1, "{errors:?}");
    assert_eq!(
        invalid_select_note(&errors),
        note("a: Leaf #(W: 9, D: 1)", code, "a: Leaf")
    );
}

#[test]
fn instantiation_note_survives_the_diagnostic_cache() {
    use miette::Diagnostic;

    let code = r#"
    module Leaf #(param W: u32 = 1) (i: input logic<8>, o: output logic) {
        assign o = i[W + 5];
    }
    module Top (i: input logic<8>, o: output logic) {
        inst l: Leaf #(W: 9) (i, o);
    }
    "#;

    let errors = analyze(code);
    let error = errors
        .iter()
        .find(|x| x.instance().is_some())
        .unwrap_or_else(|| panic!("{errors:?}"));
    let cached = vec![crate::CachedDiagnostic::from_error(error)];
    let bytes = crate::fragment_cache::capture_diagnostics(&cached).unwrap();
    let restored = crate::fragment_cache::restore_diagnostics(&bytes).unwrap();

    let related: Vec<_> = restored[0].related().unwrap().collect();
    assert_eq!(related.len(), 1);
    assert_eq!(related[0].to_string(), "reported inside l: Leaf #(W: 9)");
}

#[test]
fn error_independent_of_the_override_has_no_note_whatever_the_order() {
    // The overridden `Leaf` is converted first, but the select fails for
    // every `W`.
    let code = r#"
    module Top (i: input logic<8>, o: output logic) {
        inst l: Leaf #(W: 9) (i, o);
    }
    module Leaf #(param W: u32 = 1) (i: input logic<8>, o: output logic) {
        assign o = i[20];
    }
    "#;

    let errors = analyze(code);
    assert_eq!(invalid_select_note(&errors), None);
}

#[test]
fn interface_at_its_defaults_is_not_an_override() {
    use crate::ir::{Signature, ValueVariant};
    use crate::symbol::SymbolId;
    use veryl_parser::resource_table::StrId;

    let mut module = Signature::new(SymbolId::default());
    module.add_modport_signature(StrId::default(), Signature::new(SymbolId::default()));
    assert!(!module.has_overrides());

    let mut bus = Signature::new(SymbolId::default());
    bus.add_parameter(StrId::default(), ValueVariant::Unknown);
    let mut module = Signature::new(SymbolId::default());
    module.add_modport_signature(StrId::default(), bus);
    assert!(module.has_overrides());
}

#[test]
fn generic_arguments_are_shown_together() {
    let code = r#"
    module Gen::<A: u32, B: u32> (i: input logic<8>, o: output logic) {
        assign o = i[A + B];
    }
    module Top (i: input logic<8>, o: output logic) {
        inst u: Gen::<5, 6> (i, o);
    }
    "#;

    let errors = analyze(code);
    assert_eq!(
        invalid_select_note(&errors),
        note("u: Gen::<5, 6>", code, "u: Gen")
    );
}

#[test]
fn zero_size_from_an_override_names_the_instantiation() {
    let code = r#"
    module Sub #(param W: u32 = 1) (o: output logic) {
        var a: logic<W>;
        assign a = 0;
        assign o = a[0];
    }
    module Top (o: output logic) {
        inst u: Sub #(W: 0) (o);
    }
    "#;

    let errors = analyze(code);
    assert_eq!(
        note_of(&errors, |x| matches!(x, AnalyzerError::ZeroSize { .. })),
        note("u: Sub #(W: 0)", code, "u: Sub")
    );
}
