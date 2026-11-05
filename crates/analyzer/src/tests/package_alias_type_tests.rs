use super::*;
use crate::ir::Component;

#[test]
fn package_alias_typedef_is_valid_in_global_function_types() {
    let code = r#"
package Words::<W: u32> {
    type word_t = logic<W>;
}
alias package Five = Words::<5>;
alias package FiveChain = Five;

function f () -> Five::word_t {
    return 1;
}
function identity (x: input FiveChain::word_t) -> Five::word_t {
    var y: Five::word_t;
    y = x as FiveChain::word_t;
    return y;
}
"#;
    let errors = analyze(code);
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn package_alias_typedef_preserves_generic_arguments_and_namespace() {
    let code = r#"
package ProbeWords::<W: u32 = 3> {
    const WIDTH: u32 = W;
    type word_t = logic<WIDTH>;
}
alias package ProbeAliasFive = ProbeWords::<5>;
alias package ProbeAliasNine = ProbeWords::<9>;
alias package ProbeAliasChain = ProbeAliasFive;
package DeclScope {
    const W: u32 = 7;
    alias package ProbeAliasScoped = ProbeWords::<W>;
}
module Top (
    a: output ProbeAliasFive::word_t,
    b: output ProbeAliasNine::word_t,
    c: output ProbeAliasChain::word_t,
    d: output DeclScope::ProbeAliasScoped::word_t,
    e: output ProbeWords::<11>::word_t,
) {
    const W: u32 = 2;
    assign a = '0;
    assign b = '0;
    assign c = '0;
    assign d = '0;
    assign e = W;
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
    let mut errors = analyzer.analyze_pass1("prj", &parser.veryl);
    errors.extend(Analyzer::analyze_post_pass1());
    errors.extend(analyzer.analyze_pass2(&parser.veryl, &mut context, Some(&mut ir)));
    errors.extend(Analyzer::analyze_post_pass2(&ir));
    assert!(errors.is_empty(), "{errors:#?}");

    let module = ir
        .components
        .iter()
        .find_map(|component| match component {
            Component::Module(module) => Some(module),
            _ => None,
        })
        .unwrap();
    for (name, width) in [("a", 5), ("b", 9), ("c", 5), ("d", 7), ("e", 11)] {
        let variable = module
            .variables
            .values()
            .find(|variable| variable.path.to_string() == name)
            .unwrap();
        assert_eq!(variable.total_width(), Some(width), "{name}");
    }
}

#[test]
fn package_alias_members_are_valid_in_global_function_expressions() {
    let code = r#"
package Words::<W: u32> {
    const WIDTH: u32 = W;
    type word_t = logic<W>;
}
alias package Five = Words::<5>;
alias package FiveChain = Five;

function f () -> u32 {
    return Five::WIDTH + FiveChain::WIDTH + $bits(Five::word_t);
}
"#;
    let errors = analyze(code);
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn cast_to_alias_reports_use_site() {
    let code = r#"
module M {}
alias module AM = M;
module Top {
    var a: logic<4>;
    var b: logic<4>;
    assign a = 1;
    assign b = a as AM;
}
"#;
    let errors = analyze(code);
    let use_site = code.find("as AM").unwrap() + "as ".len();
    let location = errors
        .iter()
        .find_map(|error| match error {
            AnalyzerError::MismatchType { error_location, .. } => Some(error_location.offset()),
            _ => None,
        })
        .unwrap();
    assert_eq!(location, use_site, "{errors:#?}");
}
