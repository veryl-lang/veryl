use super::*;
use crate::ir::Component;

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
