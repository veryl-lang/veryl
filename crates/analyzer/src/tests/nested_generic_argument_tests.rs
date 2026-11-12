use super::*;

#[test]
fn nested_generic_package_constant_type_argument() {
    let sources = [
        r#"pub package Bound::<N: u32> {type Index = logic<$clog2(N)>;}"#,
        r#"pub package Geometry::<N: u32> {const ROWS: u32 = N + 1;}"#,
        r#"
        pub module Binding::<N: u32> (
            value: output prj::Bound::<Geometry::<N>::ROWS>::Index,
        ) {assign value = '1;}
        module Top (a: output logic<2>, b: output logic<3>) {
            inst first: Binding::<2> (value: a);
            inst second: Binding::<4> (value: b);
        }
        "#,
    ];
    let errors = analyze_multiple_inputs(&sources);
    assert!(errors.is_empty(), "{errors:?}");
}
