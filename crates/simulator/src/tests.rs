use crate::Simulator;
use veryl_analyzer::namespace::Namespace;
use veryl_analyzer::symbol::SymbolKind;
use veryl_analyzer::symbol_path::SymbolPath;
use veryl_analyzer::{Analyzer, AnalyzerError, definition_table, symbol_table};
use veryl_metadata::Metadata;
use veryl_parser::{Parser, resource_table};

#[track_caller]
fn analyze(code: &str) -> Vec<AnalyzerError> {
    symbol_table::clear();

    let metadata: Metadata =
        toml::from_str(&Metadata::create_default_toml("prj").unwrap()).unwrap();
    let parser = Parser::parse(&code, &"").unwrap();
    let analyzer = Analyzer::new(&metadata);

    let mut errors = vec![];
    errors.append(&mut analyzer.analyze_pass1(&"prj", &"", &parser.veryl));
    errors.append(&mut Analyzer::analyze_post_pass1());
    errors.append(&mut analyzer.analyze_pass2(&"prj", &"", &parser.veryl));
    let info = Analyzer::analyze_post_pass2();
    errors.append(&mut analyzer.analyze_pass3(&"prj", &"", &parser.veryl, &info));
    dbg!(&errors);
    errors
}

#[test]
fn simple_sim() {
    let code = r#"
    module Top (
        a: input  logic<32>,
        b: input  logic<32>,
        c: output logic<32>,
    ) {
        assign c = a + b;
    }
    "#;

    // Call Veryl compiler
    let errors = analyze(code);
    assert!(errors.is_empty());

    let top = resource_table::insert_str("Top");
    let namespace = Namespace::default();
    let path = SymbolPath::new(&[top]);

    // get top module definition
    if let Ok(symbol) = symbol_table::resolve((&path, &namespace)) {
        if let SymbolKind::Module(x) = &symbol.found.kind {
            let top_define = definition_table::get(x.definition);
            dbg!(top_define);
        }
    }

    // get port/variable symbol information
    for symbol in symbol_table::get_all() {
        match symbol.kind {
            SymbolKind::Port(x) => {
                dbg!(x);
            }
            SymbolKind::Variable(x) => {
                dbg!(x);
            }
            _ => (),
        }
    }

    // Create new simulator instance specifing "Top" as top module
    let mut sim = Simulator::new("Top");

    // Set values to input ports
    sim.set("a", 10);
    sim.set("b", 20);

    // Execute 1 clock cycle simulation
    sim.step();

    // Get values from output ports
    //assert_eq!(sim.get("c"), 30);
}
