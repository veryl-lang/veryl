use super::*;
use crate::ir::{
    Component, Comptime, Declaration, Expression, Factor, Module, Shape, ShapeRef, Type, TypeKind,
    VarId, VarIndex, VarKind, VarPath, VarSelect, Variable,
};
use crate::symbol::Affiliation;
use crate::value::Value;
use veryl_parser::token_range::TokenRange;

fn array_member() -> Variable {
    let mut r#type = Type::new(TypeKind::Logic);
    r#type.set_concrete_width(Shape::new(vec![Some(8)]));
    r#type.array = Shape::new(vec![Some(3)]);
    let values = [0x11, 0x22, 0x33]
        .into_iter()
        .map(|x| Value::new(x, 8, false))
        .collect();
    let mut variable = Variable::new(
        VarId::default(),
        "outer.inner.data".parse().unwrap(),
        VarKind::Variable,
        r#type,
        values,
        Affiliation::Module,
        &TokenRange::default(),
        usize::MAX,
    );
    // Distinct masks catch reordering and accidental repetition of one element.
    variable.assigned = vec![1u32.into(), 2u32.into(), 4u32.into()];
    variable
}

#[test]
fn prepend_empty_array_is_identity() {
    let mut variable = array_member();
    let original = variable.clone();

    variable.prepend_array_at_path(ShapeRef::new(&[]), 2);

    assert_eq!(variable.r#type.array, original.r#type.array);
    assert_eq!(variable.array_path_offsets, original.array_path_offsets);
    assert_eq!(variable.value, original.value);
    assert_eq!(variable.assigned, original.assigned);
}

#[test]
fn prepend_known_array_repeats_the_original_block() {
    for (outer, repetitions) in [(vec![Some(1)], 1), (vec![Some(2), Some(2)], 4)] {
        let mut variable = array_member();
        let original = variable.clone();

        variable.prepend_array_at_path(ShapeRef::new(&outer), 2);

        let mut expected_shape = outer.clone();
        expected_shape.push(Some(3));
        let mut expected_offsets = vec![2; outer.len()];
        expected_offsets.push(0);
        let expected_values: Vec<_> = (0..repetitions)
            .flat_map(|_| original.value.iter().cloned())
            .collect();
        let expected_assigned: Vec<_> = (0..repetitions)
            .flat_map(|_| original.assigned.iter().cloned())
            .collect();

        assert_eq!(variable.r#type.array.as_slice(), expected_shape.as_slice());
        assert_eq!(variable.array_path_offsets, expected_offsets);
        assert_eq!(variable.r#type.total_array(), Some(3 * repetitions));
        assert_eq!(variable.value, expected_values);
        assert_eq!(variable.assigned, expected_assigned);
    }
}

#[test]
fn prepend_unknown_array_keeps_rank_without_expanding_storage() {
    for outer in [vec![None], vec![Some(2), None], vec![None, Some(2)]] {
        let mut variable = array_member();
        let original = variable.clone();

        variable.prepend_array_at_path(ShapeRef::new(&outer), 2);

        let mut expected_shape = outer.clone();
        expected_shape.push(Some(3));
        let mut expected_offsets = vec![2; outer.len()];
        expected_offsets.push(0);

        assert_eq!(variable.r#type.array.as_slice(), expected_shape.as_slice());
        assert_eq!(variable.array_path_offsets, expected_offsets);
        assert_eq!(variable.r#type.total_array(), None);
        assert_eq!(variable.r#type.total_width(), Some(8));
        assert_eq!(variable.value, original.value);
        assert_eq!(variable.assigned, original.assigned);

        // The retained storage must not become a concrete element of the
        // unknown-sized array just because the requested indices are known.
        let index = vec![0; expected_shape.len()];
        assert!(variable.get_value(&index).is_none());
        assert!(!variable.set_value(&index, Value::new(0xff, 8, false), None));
        assert_eq!(variable.value, original.value);
    }
}

#[test]
fn prepend_nested_arrays_preserves_each_dimension_owner() {
    let mut variable = array_member();

    variable.prepend_array_at_path(ShapeRef::new(&[None]), 1);
    variable.prepend_array_at_path(ShapeRef::new(&[Some(2)]), 2);

    assert_eq!(variable.r#type.array.as_slice(), &[Some(2), None, Some(3)]);
    assert_eq!(variable.array_path_offsets, vec![2, 1, 0]);
    assert_eq!(variable.r#type.total_array(), None);
}

#[test]
fn prepend_known_array_uses_separate_value_and_assigned_lengths() {
    let mut variable = array_member();
    // A single initial-value template can describe all three member elements,
    // while assignment coverage is still stored per element.
    variable.value.truncate(1);
    let original = variable.clone();

    variable.prepend_array_at_path(ShapeRef::new(&[Some(2)]), 1);

    let expected_values: Vec<_> = (0..2)
        .flat_map(|_| original.value.iter().cloned())
        .collect();
    let expected_assigned: Vec<_> = (0..2)
        .flat_map(|_| original.assigned.iter().cloned())
        .collect();
    assert_eq!(variable.r#type.array.as_slice(), &[Some(2), Some(3)]);
    assert_eq!(variable.array_path_offsets, vec![1, 0]);
    assert_eq!(variable.value, expected_values);
    assert_eq!(variable.assigned, expected_assigned);
}

#[track_caller]
fn analyze_connection_ir(code: &str) -> Ir {
    symbol_table::clear();
    attribute_table::clear();
    doc_comment_table::clear();

    let metadata = Metadata::create_default("prj").unwrap();
    let parser = Parser::parse(code, &"").unwrap();
    let analyzer = Analyzer::new(&metadata);
    let mut context = Context::default();
    let mut ir = Ir::default();
    let mut errors = vec![];
    errors.append(&mut analyzer.analyze_pass1("prj", &parser.veryl));
    errors.append(&mut Analyzer::analyze_post_pass1());
    errors.append(&mut analyzer.analyze_pass2(&parser.veryl, &mut context, Some(&mut ir)));
    errors.append(&mut Analyzer::analyze_post_pass2(&ir));
    assert!(errors.is_empty(), "{errors:#?}\n{code}");
    ir
}

struct ArrayCase<'a> {
    declaration: &'a str,
    selection: &'a str,
    shape: &'a [Option<usize>],
    indices: &'a [Option<usize>],
}

#[track_caller]
fn assert_member_connection(
    top: &Module,
    member: &str,
    width: usize,
    case: &ArrayCase<'_>,
    (id, index, select, comptime): (VarId, &VarIndex, &VarSelect, &Comptime),
) {
    let variable = top.variables.get(&id).expect("missing parent variable");
    assert_eq!(variable.path.to_string(), format!("ifs.{member}"));
    assert_eq!(variable.r#type.array.as_slice(), case.shape, "{member}");
    assert_eq!(variable.array_path_offsets, vec![1; case.shape.len()]);
    assert_eq!(variable.r#type.total_array(), None);
    assert_eq!(variable.r#type.total_width(), Some(width), "{member}");

    // An interface element, rather than the whole instance array, is connected.
    assert!(comptime.r#type.array.is_empty(), "{member}");
    assert_eq!(comptime.r#type.total_width(), Some(width), "{member}");
    assert_eq!(index.0.len(), case.indices.len(), "{member}");
    let mut context = Context::default();
    for (expr, expected) in index.0.iter().zip(case.indices) {
        let actual = expr.eval_value(&mut context).and_then(|v| v.to_usize());
        assert_eq!(actual, *expected, "{member}: {index}");
    }

    // A struct may use an explicit full-width packed range. Check the covered
    // bits, not the spelling of the select: [N] must not become a bit select.
    assert_eq!(
        select.eval_value(&mut context, &comptime.r#type, false),
        Some((width - 1, 0)),
        "{member}: unexpected packed selection {select}"
    );
}

#[track_caller]
fn check_modport_connection(case: &ArrayCase<'_>) {
    for view in ["", ".master"] {
        let code = format!(
            r#"
            package p {{
                const N: u32 = $sv::foo_pkg::BAR;
                enum cmd_e {{ A, B, }}
                struct payload_t {{ tag: logic<8>, data: logic<8>, }}
            }}
            interface my_if {{
                var request: logic<8>;
                var cmd: p::cmd_e;
                var payload: p::payload_t;
                var data: logic<8>;
                modport master {{
                    request: input,
                    cmd: output,
                    payload: output,
                    data: output,
                }}
            }}
            module leaf (m_if: modport my_if::master,) {{
                always_comb {{
                    m_if.cmd = p::cmd_e::A;
                    m_if.payload = p::payload_t'{{tag: 8'haa, data: 8'h55}};
                    m_if.data = m_if.request;
                }}
            }}
            module top (i: input logic<8>,) {{
                inst ifs: my_if [{array}];
                inst u: leaf (m_if: ifs{selection}{view},);
                assign ifs{selection}.request = i;
            }}
            "#,
            array = case.declaration,
            selection = case.selection,
        );
        let ir = analyze_connection_ir(&code);
        let top = ir
            .components
            .iter()
            .find_map(|component| match component {
                Component::Module(module) if module.name.to_string() == "top" => Some(module),
                _ => None,
            })
            .expect("missing top module");
        let inst = top
            .declarations
            .iter()
            .find_map(|declaration| match declaration {
                Declaration::Inst(inst) if inst.name.to_string() == "u" => Some(inst),
                _ => None,
            })
            .expect("missing leaf instance");
        let Component::Module(child) = inst.component.as_ref() else {
            panic!("leaf must lower to a module");
        };

        // No connection may disappear, even if unknown array sizes suppress
        // the unassigned-variable diagnostic that would otherwise expose it.
        assert_eq!(inst.inputs.len(), 1, "{code}");
        assert_eq!(inst.outputs.len(), 3, "{code}");
        for (member, width) in [("cmd", 1), ("payload", 16), ("data", 8)] {
            let path: VarPath = format!("m_if.{member}").parse().unwrap();
            let port_id = child.ports.get(&path).expect("missing child output port");
            let output = inst
                .outputs
                .iter()
                .find(|output| output.id == *port_id)
                .expect("missing output connection");
            assert_eq!(output.dst.len(), 1, "{member}: {code}");
            let dst = &output.dst[0];
            assert_eq!(dst.path.to_string(), format!("ifs.{member}"));
            assert_member_connection(
                top,
                member,
                width,
                case,
                (dst.id, &dst.index, &dst.select, &dst.comptime),
            );
        }

        let request: VarPath = "m_if.request".parse().unwrap();
        let port_id = child.ports.get(&request).expect("missing child input port");
        let input = inst
            .inputs
            .iter()
            .find(|input| input.id == *port_id)
            .expect("missing input connection");
        let expr = input.single().expect("input must be one interface element");
        let Expression::Term(factor) = expr else {
            panic!("input must retain a variable reference: {expr:?}");
        };
        let Factor::Variable(id, index, select, comptime) = factor.as_ref() else {
            panic!("input must not become an unknown factor: {factor:?}");
        };
        assert_member_connection(top, "request", 8, case, (*id, index, select, comptime));
    }
}

#[test]
fn unknown_interface_array_connections_preserve_member_ir() {
    for (selection, indices) in [("[p::N]", &[None][..]), ("[0]", &[Some(0)][..])] {
        check_modport_connection(&ArrayCase {
            declaration: "p::N + 1",
            selection,
            shape: &[None],
            indices,
        });
    }
}

#[test]
fn mixed_interface_array_dimensions_preserve_select_order() {
    for case in [
        ArrayCase {
            declaration: "2, p::N + 1",
            selection: "[1][p::N]",
            shape: &[Some(2), None],
            indices: &[Some(1), None],
        },
        ArrayCase {
            declaration: "p::N + 1, 2",
            selection: "[p::N][1]",
            shape: &[None, Some(2)],
            indices: &[None, Some(1)],
        },
    ] {
        check_modport_connection(&case);
    }
}
