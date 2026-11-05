use super::*;
use crate::ir::Component;
use crate::value::Value;

pub(super) fn evaluate_contract_constant(code: &str) -> Value {
    let metadata = Metadata::create_default("prj").unwrap();
    let analyzer = Analyzer::new(&metadata);
    analyzer.clear();
    let parser = Parser::parse(code, &"").unwrap();
    let mut context = Context::default();
    let mut ir = Ir::default();
    let mut errors = analyzer.analyze_pass1("prj", &parser.veryl);
    errors.extend(Analyzer::analyze_post_pass1());
    errors.extend(analyzer.analyze_pass2(&parser.veryl, &mut context, Some(&mut ir)));
    errors.extend(Analyzer::analyze_post_pass2(&ir));
    assert!(errors.is_empty(), "{code}\n{errors:#?}");
    let Component::Module(module) = &ir.components[0] else {
        panic!("expected module");
    };
    module
        .variables
        .values()
        .find(|variable| variable.path.to_string() == "RESULT")
        .unwrap()
        .get_value(&[])
        .unwrap()
        .clone()
}

#[test]
fn aggregate_contract_nested_struct_union_array_coordinates() {
    const FIELDS: [u64; 16] = [0, 1, 2, 3, 1, 3, 0, 2, 3, 2, 1, 0, 2, 0, 3, 1];
    let mut words = [0u64; 2];
    for (array, word) in words.iter_mut().enumerate() {
        for outer in 0..2 {
            for union in 0..2 {
                for inner in 0..2 {
                    let low = outer * 25 + 4 + union * 10 + inner * 5 + 3;
                    *word |= FIELDS[array * 8 + outer * 4 + union * 2 + inner] << low;
                }
            }
        }
    }
    for array in 0..2 {
        for outer in 0..2 {
            for union in 0..2 {
                for inner in 0..2 {
                    let code = format!(
                        r#"
module Top {{
    struct Leaf {{ hi: logic<2>, lo: logic<3>, }}
    union View {{ fields: Leaf<2>, raw: logic<10>, }}
    struct Packet {{ head: logic, body: View<2>, tail: logic<4>, }}
    function probe(a: input u32, o: input u32, u: input u32, i: input u32) -> logic<2> {{
        var storage: Packet<2>[2];
        storage = '{{50'h{first:x},50'h{second:x}}};
        return storage[a][o].body[u].fields[i].hi;
    }}
    const RESULT: logic<2> = probe({array},{outer},{union},{inner});
}}
"#,
                        first = words[0],
                        second = words[1]
                    );
                    let value = evaluate_contract_constant(&code);
                    assert_eq!(
                        (value.payload_u128(), value.mask_xz_u128()),
                        (FIELDS[array * 8 + outer * 4 + union * 2 + inner] as u128, 0),
                        "{code}"
                    );
                }
            }
        }
    }
}

#[test]
fn coordinate_contract_packed_and_unpacked_source_paths() {
    for row in 0..2 {
        for column in 0..3 {
            let code = format!(
                r#"
module Top {{
    function probe(i: input u32, j: input u32) -> logic<4> {{
        var storage: logic<2, 3, 4>;
        storage = 24'h654321;
        return storage[i][j];
    }}
    const RESULT: logic<4> = probe({row}, {column});
}}
"#
            );
            let result = evaluate_contract_constant(&code);
            assert_eq!(
                (result.payload_u128(), result.mask_xz_u128()),
                (row * 3 + column + 1, 0),
                "{code}"
            );
        }
    }
    for (select, expected) in [("1+:2", 1), ("2-:2", 1), ("1 step 2", 2), ("3:2", 2)] {
        let code = format!(
            r#"
module Top {{
    function probe(a: input logic<24>) -> logic<8> {{ return a[{select}]; }}
    const RESULT: logic<8> = probe(24'h65436b);
}}
"#
        );
        let result = evaluate_contract_constant(&code);

        assert_eq!(
            (result.payload_u128(), result.mask_xz_u128()),
            (expected, 0),
            "{code}"
        );
    }
    for (select, base, expected) in [
        ("i+:2", 1, 3),
        ("i-:2", 2, 3),
        ("i step 2", 1, 4),
        ("1:2", 0, 3),
    ] {
        let code = format!(
            r#"
module Top {{
    function probe(i: input u32) -> logic<4> {{
        var storage: logic<4>[6];
        storage = '{{1,2,3,4,5,6}};
        var selected: logic<4>[2];
        selected = storage[{select}];
        return selected[1];
    }}
    const RESULT: logic<4> = probe({base});
}}
"#
        );
        let result = evaluate_contract_constant(&code);
        assert_eq!(
            (result.payload_u128(), result.mask_xz_u128()),
            (expected, 0),
            "{code}"
        );
    }
}

#[test]
fn shape_contract_multidimensional_values_and_prefixes() {
    for row in 0..2 {
        for column in 0..3 {
            let code = format!(
                r#"
module Top {{
    function probe(i: input u32, j: input u32) -> logic<8> {{
        var storage: logic<8>[2, 3];
        storage = '{{'{{8'd1, 8'd2, 8'd3}}, '{{8'd4, 8'd5, 8'd6}}}};
        return storage[i][j];
    }}
    const RESULT: logic<8> = probe({row}, {column});
}}
"#
            );
            let result = evaluate_contract_constant(&code);
            assert_eq!(result.payload_u128(), row * 3 + column + 1, "{code}");
            assert_eq!(result.mask_xz_u128(), 0);
        }
        let code = format!(
            r#"
module Top {{
    function probe(i: input u32) -> logic<8> {{
        var storage: logic<8>[2, 3];
        storage = '{{'{{8'd1, 8'd2, 8'd3}}, '{{8'd4, 8'd5, 8'd6}}}};
        var selected: logic<8>[3];
        selected = storage[i];
        return selected[2];
    }}
    const RESULT: logic<8> = probe({row});
}}
"#
        );
        let result = evaluate_contract_constant(&code);
        assert_eq!(result.payload_u128(), row * 3 + 3, "{code}");
        assert_eq!(result.mask_xz_u128(), 0);
    }
}

#[test]
fn aggregate_contract_dynamic_member_indices_do_not_alias_adjacent_fields() {
    for index in ["0", "1", "2", "4'bx", "4'bz"] {
        let code = format!(
            "module Top {{struct Leaf {{hi:logic<2>,lo:logic<3>,}}struct Packet {{head:logic,body:Leaf<2>,tail:logic<2>,}}function probe(i:input logic<4>)->logic<2>{{var storage:Packet;storage=13'h1fff;return storage.body[i].hi;}}const RESULT:logic<2> = probe({index});}}"
        );
        let value = evaluate_contract_constant(&code);
        let expected = if matches!(index, "0" | "1") {
            (3, 0)
        } else {
            (0, 3)
        };
        assert_eq!(
            (value.payload_u128(), value.mask_xz_u128()),
            expected,
            "index={index}"
        );
    }
}
