use super::*;
use miette::{Diagnostic, Severity};

fn assert_incompatible(code: &str) {
    let errors = analyze(code);
    assert_eq!(errors.len(), 1, "{code}\n{errors:#?}");
    assert!(matches!(
        errors[0],
        AnalyzerError::IncompatibleAssignment { .. }
    ));
    assert_eq!(errors[0].severity(), Some(Severity::Error));
}

#[test]
fn incompatible_array_ports_are_errors() {
    // Equal element/bit counts do not make packed and unpacked arrays
    // assignment compatible (IEEE 1800-2023 7.6, 23.3.3).
    for (formal, actual, drive) in [
        ("logic<4, 8>", "logic<8>[4]", "assign p = '0;"),
        (
            "logic<8>[4]",
            "logic<4, 8>",
            "for i in 0..4 :g { assign p[i] = '0; }",
        ),
        ("logic<8>", "logic<8>[1]", "assign p = '0;"),
    ] {
        for direction in ["input", "output"] {
            for connection in ["p: p", "p"] {
                let body = if direction == "output" { drive } else { "" };
                assert_incompatible(&format!(
                    "module Leaf(p: {direction} {formal}) {{ {body} }}
                     module Top(p: {direction} {actual}) {{ inst u: Leaf({connection}); }}"
                ));
            }
        }
    }
}

#[test]
fn output_connections_check_the_actual_destination_type() {
    for connection in ["y: y", "y"] {
        for (source_type, drive) in [("p::E", "p::E::A"), ("bit<8>", "'0")] {
            let errors = analyze(&format!(
                "package p {{ enum E: bit<8> {{ A = 8'd0, }} }}
                 module Leaf(y: output {source_type}) {{ assign y = {drive}; }}
                 module Top(y: output logic<8>) {{ inst u: Leaf({connection}); }}"
            ));
            assert!(errors.is_empty(), "{errors:#?}");
        }

        // Enum -> integral is legal, while the opposite direction needs a
        // cast (IEEE 1800-2023 6.19.3, 6.22.3, 23.3.3).
        assert_incompatible(&format!(
            "package p {{ enum E: bit<8> {{ A = 8'd0, }} }}
             module Leaf(y: output logic<8>) {{ assign y = '0; }}
             module Top(y: output p::E) {{ inst u: Leaf({connection}); }}"
        ));

        let errors = analyze(&format!(
            "module Leaf(y: output logic<8>) {{ assign y = '0; }}
             module Top(y: output bit<8>) {{ inst u: Leaf({connection}); }}"
        ));
        assert_eq!(errors.len(), 1, "{errors:#?}");
        assert!(matches!(
            &errors[0],
            AnalyzerError::MismatchAssignment { src, dst, .. }
                if src == "logic<8>" && dst == "bit<8>"
        ));
    }
}

#[test]
fn language_type_violations_are_errors() {
    for code in [
        "module Top(a: input logic<8>[4], y: output logic<4, 8>) { assign y = a; }",
        // Same shape, but element types are not equivalent (7.6, 6.22.2).
        "module Top(a: input logic<8>[2], y: output bit<8>[2]) { assign y = a; }",
        "module Top { const T: type = 0; }",
        "module Top::<T: type = 0> {}",
        // X/Z enumerators require an explicit 4-state base (6.19).
        "module Top { enum E: bit<1> { A = 1'bx, } }",
        // Enum destinations require an explicit cast (6.19.3).
        "package p { enum E: bit<8> { A = 8'd0, } }
         module Top(a: input logic<8>, y: output p::E) { assign y = a; }",
        // String objects differ from string literals (6.16).
        "module Top { const S: string = \"A\"; let _a: bit<8> = S; }",
        // A named interface port requires that interface type (23.3.3.4).
        "interface A { var a: logic; modport mp { a: input, } }
         interface B { var a: logic; modport mp { a: input, } }
         module Leaf(p: modport A::mp) {}
         module Top { inst b: B; assign b.a = '0; inst u: Leaf(p: b); }",
        // Two explicitly selected modports must match (25.5).
        "interface A { var a: logic; modport mp { a: input, } modport other { a: input, } }
         module Leaf(p: modport A::mp) {}
         module Top(p: modport A::other) { inst u: Leaf(p: p); }",
    ] {
        assert_incompatible(code);
    }
}

#[test]
fn legal_scalar_conversions_remain_warnings() {
    // Integral and real scalar conversions are defined by SV; losing X/Z or
    // narrowing a clock binding is a code smell, not an incompatible type.
    for code in [
        "module Top(a: input logic<8>, y: output bit<8>) { assign y = a; }",
        "module Top { let _a: u32 = 'x; }",
        "module Top(a: input logic<32>, y: output f64) { assign y = a; }",
        "module Top(a: input logic<32>) { let _c: clock = a; }",
        "module Top { let _a: bit<8> = \"A\"; }",
        "module Top { let _a: bit<8> = (\"A\"); }",
    ] {
        let errors = analyze(code);
        assert_eq!(errors.len(), 1, "{code}\n{errors:#?}");
        assert!(matches!(
            errors[0],
            AnalyzerError::MismatchAssignment { .. }
        ));
        assert_eq!(errors[0].severity(), Some(Severity::Warning));
    }
}

#[test]
fn compatible_or_unknown_types_are_not_rejected() {
    for (formal, actual, expr) in [
        ("logic<8>[4]", "logic<8>[4]", "a"),
        ("logic<4, 8>", "logic<32>", "a"),
        ("logic<8>", "logic<8>[4]", "a[0]"),
        ("logic<8>[2]", "logic<8>[4]", "a[0+:2]"),
        ("logic<4, 8>", "logic<8>[4]", "{a[3], a[2], a[1], a[0]}"),
        ("logic<4, 8>", "logic<8>[4]", "'{a[3], a[2], a[1], a[0]}"),
        ("logic<8>[4]", "logic<8>[$sv::N]", "a"),
    ] {
        let errors = analyze(&format!(
            "module Leaf(p: input {formal}) {{}}
             module Top(a: input {actual}) {{ inst u: Leaf(p: {expr}); }}"
        ));
        assert!(errors.is_empty(), "{errors:#?}");
    }
}
