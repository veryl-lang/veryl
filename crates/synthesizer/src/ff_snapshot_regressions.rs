use crate::regression_support::eval;
use std::collections::HashMap;
use veryl_analyzer::{Analyzer, Context, ir};
use veryl_parser::{Parser, resource_table};
#[test]
fn later_ff_reads_observe_pre_edge_state() {
    let source = "module Top(clk:input clock,rst:input reset,q:output logic<2>,previous:output logic<2>){var state:logic<2>;var prev:logic<2>;always_ff{if_reset{state=1;prev=0;}else{state=state+1;prev=state;}}assign q=state;assign previous=prev;}";
    let metadata = veryl_metadata::Metadata::create_default("prj").unwrap();
    let analyzer = Analyzer::new(&metadata);
    analyzer.clear();
    let parser = Parser::parse(source, &"").unwrap();
    let mut context = Context::default();
    let mut ir = ir::Ir::default();
    let mut errors = analyzer.analyze_pass1("prj", &parser.veryl);
    errors.extend(Analyzer::analyze_post_pass1());
    errors.extend(analyzer.analyze_pass2(&parser.veryl, &mut context, Some(&mut ir)));
    errors.extend(Analyzer::analyze_post_pass2(&ir));
    assert!(errors.is_empty(), "{errors:?}");
    let gate = crate::build_gate_ir(&ir, resource_table::insert_str("Top"))
        .unwrap()
        .module;
    let mut inputs = HashMap::new();
    for p in &gate.ports {
        let name = resource_table::get_str_value(p.name).unwrap();
        if name == "clk" || name == "rst" {
            for &n in &p.nets {
                inputs.insert(n, name == "clk");
            }
        }
    }
    for f in &gate.ffs {
        inputs.insert(f.q, f.reset_value);
    }
    for f in &gate.ffs {
        let d = eval(&gate, f.d, &inputs, &mut vec![None; gate.nets.len()], 0);
        let output = gate
            .ports
            .iter()
            .find(|p| resource_table::get_str_value(p.name).unwrap() == "previous")
            .unwrap();
        if output.nets.contains(&f.q) {
            let i = output.nets.iter().position(|n| *n == f.q).unwrap();
            assert_eq!(d, i == 0);
        }
    }
}
