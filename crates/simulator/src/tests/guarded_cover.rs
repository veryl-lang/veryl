use crate::ir::declaration::stable_topo_sort_with_blocks;
use crate::ir::{
    ExpressionContext, Op, ProtoAssignDynamicStatement, ProtoAssignStatement, ProtoExpression,
    ProtoIfStatement, ProtoStatement, VarOffset,
};

const GUARD: VarOffset = VarOffset::Comb(8);
const TEMP: VarOffset = VarOffset::Comb(24);
const OUTPUT: VarOffset = VarOffset::Comb(32);
const INPUT: VarOffset = VarOffset::Comb(40);
const OTHER: VarOffset = VarOffset::Comb(48);
const MASK: VarOffset = VarOffset::Comb(56);
// Three 8-byte elements at 64, 72 and 80.
const ARRAY: VarOffset = VarOffset::Comb(64);
const MIDDLE: VarOffset = VarOffset::Comb(72);

fn read(var_offset: VarOffset) -> ProtoExpression {
    ProtoExpression::Variable {
        var_offset,
        select: None,
        dynamic_select: None,
        width: 8,
        var_full_width: 8,
        expr_context: ExpressionContext {
            width: 8,
            signed: false,
        },
    }
}

fn assign(dst: VarOffset, src: VarOffset) -> ProtoStatement {
    ProtoStatement::Assign(ProtoAssignStatement {
        dst,
        dst_width: 8,
        select: None,
        dynamic_select: None,
        rhs_select: None,
        expr: read(src),
        dst_ff_current_offset: 0,
        comb_direct: false,
        token: Default::default(),
    })
}

fn branch(guard: VarOffset, body: Vec<ProtoStatement>, take_true: bool) -> ProtoStatement {
    let (true_side, false_side) = if take_true {
        (body, vec![])
    } else {
        (vec![], body)
    };
    ProtoStatement::If(ProtoIfStatement {
        cond: Some(read(guard)),
        true_side,
        false_side,
    })
}

fn pieces(take_true: bool) -> Vec<ProtoStatement> {
    vec![
        branch(GUARD, vec![assign(TEMP, INPUT)], take_true),
        branch(GUARD, vec![assign(OUTPUT, TEMP)], take_true),
        branch(OTHER, vec![assign(TEMP, OTHER)], true),
    ]
}

fn hint(statements: Vec<ProtoStatement>) -> Option<usize> {
    let blocks = vec![0; statements.len()];
    stable_topo_sort_with_blocks(statements, &blocks).1
}

#[test]
fn same_guard_covers_either_branch() {
    for take_true in [false, true] {
        assert_eq!(hint(pieces(take_true)), Some(1));
    }
}

#[test]
fn unrelated_guard_does_not_cover_a_read() {
    let mut statements = pieces(true);
    statements[1] = branch(OTHER, vec![assign(OUTPUT, TEMP)], true);
    assert_ne!(hint(statements), Some(1));
}

#[test]
fn changed_guard_does_not_cover_a_read() {
    let mut statements = pieces(true);
    statements.insert(1, assign(GUARD, OTHER));
    assert_ne!(hint(statements), Some(1));
    for piece in [0, 1] {
        let mut statements = pieces(true);
        let ProtoStatement::If(conditional) = &mut statements[piece] else {
            unreachable!()
        };
        conditional.true_side.push(assign(GUARD, OTHER));
        assert_ne!(hint(statements), Some(1));
    }
}

#[test]
fn dynamic_write_to_an_interior_guard_does_not_prove_cover() {
    let mut statements = pieces(true);
    statements.insert(
        1,
        ProtoStatement::AssignDynamic(ProtoAssignDynamicStatement {
            dst_base: VarOffset::Comb(0),
            dst_stride: 8,
            dst_num_elements: 3,
            dst_index_expr: read(OTHER),
            dst_width: 8,
            select: None,
            dynamic_select: None,
            rhs_select: None,
            expr: read(INPUT),
            dst_ff_current_base_offset: 0,
            comb_direct: false,
        }),
    );
    assert_ne!(hint(statements), Some(1));
}

#[test]
fn different_guard_bits_do_not_prove_cover() {
    let mut statements = pieces(true);
    for (piece, bit) in [(0, 0), (1, 1)] {
        let ProtoStatement::If(conditional) = &mut statements[piece] else {
            unreachable!()
        };
        let Some(ProtoExpression::Variable {
            select,
            width,
            expr_context,
            ..
        }) = &mut conditional.cond
        else {
            unreachable!()
        };
        *select = Some((bit, bit));
        *width = 1;
        expr_context.width = 1;
    }
    assert_ne!(hint(statements), Some(1));
}

#[test]
fn opposite_branch_read_is_not_guarded_by_the_write() {
    let mut statements = pieces(true);
    let ProtoStatement::If(conditional) = &mut statements[1] else {
        unreachable!()
    };
    conditional.false_side.push(assign(OUTPUT, TEMP));
    assert_ne!(hint(statements), Some(1));
}

#[test]
fn partial_or_optional_write_does_not_cover_the_whole_read() {
    let mut statements = pieces(true);
    let ProtoStatement::If(conditional) = &mut statements[0] else {
        unreachable!()
    };
    let ProtoStatement::Assign(write) = &mut conditional.true_side[0] else {
        unreachable!()
    };
    write.select = Some((3, 0));
    assert_ne!(hint(statements), Some(1));

    let mut statements = pieces(true);
    statements[0] = branch(
        GUARD,
        vec![branch(OTHER, vec![assign(TEMP, INPUT)], true)],
        true,
    );
    assert_ne!(hint(statements), Some(1));
}

#[test]
fn guarded_partial_writes_can_cover_the_whole_read() {
    let mut statements = pieces(true);
    let mut low = assign(TEMP, INPUT);
    let mut high = assign(TEMP, INPUT);
    let ProtoStatement::Assign(write) = &mut low else {
        unreachable!()
    };
    write.select = Some((3, 0));
    let ProtoStatement::Assign(write) = &mut high else {
        unreachable!()
    };
    write.select = Some((7, 4));
    statements[0] = branch(GUARD, vec![low, high], true);
    assert_eq!(hint(statements), Some(1));
}

#[test]
fn reading_the_guard_itself_is_not_covered_by_its_body() {
    let statements = vec![
        branch(TEMP, vec![assign(TEMP, INPUT)], true),
        branch(TEMP, vec![assign(OUTPUT, TEMP)], true),
        branch(OTHER, vec![assign(TEMP, OTHER)], true),
    ];
    assert_ne!(hint(statements), Some(1));
}

fn context() -> ExpressionContext {
    ExpressionContext {
        width: 8,
        signed: false,
    }
}

fn read_element(index: VarOffset) -> ProtoExpression {
    ProtoExpression::DynamicVariable {
        base_offset: ARRAY,
        stride: 8,
        element_native_bytes: 8,
        index_expr: Box::new(read(index)),
        num_elements: 3,
        select: None,
        dynamic_select: None,
        width: 8,
        expr_context: context(),
    }
}

fn masked_guard() -> ProtoExpression {
    ProtoExpression::Binary {
        x: Box::new(read(GUARD)),
        op: Op::BitAnd,
        y: Box::new(read(MASK)),
        width: 8,
        expr_context: context(),
    }
}

fn guarded(cond: ProtoExpression, true_side: Vec<ProtoStatement>) -> ProtoStatement {
    ProtoStatement::If(ProtoIfStatement {
        cond: Some(cond),
        true_side,
        false_side: vec![],
    })
}

#[test]
fn compound_guard_covers_until_an_input_is_rewritten() {
    let compound = || {
        vec![
            guarded(masked_guard(), vec![assign(TEMP, INPUT)]),
            guarded(masked_guard(), vec![assign(OUTPUT, TEMP)]),
            branch(OTHER, vec![assign(TEMP, OTHER)], true),
        ]
    };
    assert_eq!(hint(compound()), Some(1));

    let mut statements = compound();
    statements.insert(1, assign(MASK, OTHER));
    assert_ne!(hint(statements), Some(1));

    let mut statements = compound();
    let ProtoStatement::If(conditional) = &mut statements[0] else {
        unreachable!()
    };
    conditional.true_side.push(assign(MASK, OTHER));
    assert_ne!(hint(statements), Some(1));
}

#[test]
fn guard_with_another_operand_does_not_prove_cover() {
    let mut statements = pieces(true);
    statements[1] = guarded(masked_guard(), vec![assign(OUTPUT, TEMP)]);
    assert_ne!(hint(statements), Some(1));
}

#[test]
fn runtime_indexed_guard_covers_until_any_element_is_rewritten() {
    let pieces = |between: Vec<ProtoStatement>| {
        let mut statements = vec![guarded(read_element(OTHER), vec![assign(TEMP, INPUT)])];
        statements.extend(between);
        statements.push(guarded(read_element(OTHER), vec![assign(OUTPUT, TEMP)]));
        statements.push(branch(OTHER, vec![assign(TEMP, OTHER)], true));
        statements
    };
    assert_eq!(hint(pieces(vec![])), Some(1));
    // The middle element is not one of the two the offset gather names.
    assert_ne!(hint(pieces(vec![assign(MIDDLE, INPUT)])), Some(1));
    assert_ne!(
        hint(pieces(vec![ProtoStatement::AssignDynamic(
            ProtoAssignDynamicStatement {
                dst_base: ARRAY,
                dst_stride: 8,
                dst_num_elements: 3,
                dst_index_expr: read(INPUT),
                dst_width: 8,
                select: None,
                dynamic_select: None,
                rhs_select: None,
                expr: read(INPUT),
                dst_ff_current_base_offset: 0,
                comb_direct: false,
            }
        )])),
        Some(1)
    );
}

#[test]
fn opposite_arm_runtime_index_reads_the_temporary() {
    let statements = vec![
        branch(GUARD, vec![assign(MIDDLE, INPUT)], true),
        ProtoStatement::If(ProtoIfStatement {
            cond: Some(read(GUARD)),
            true_side: vec![assign(OUTPUT, MIDDLE)],
            false_side: vec![ProtoStatement::Assign(ProtoAssignStatement {
                dst: OUTPUT,
                dst_width: 8,
                select: None,
                dynamic_select: None,
                rhs_select: None,
                expr: read_element(OTHER),
                dst_ff_current_offset: 0,
                comb_direct: false,
                token: Default::default(),
            })],
        }),
        branch(OTHER, vec![assign(MIDDLE, OTHER)], true),
    ];
    assert_ne!(hint(statements), Some(1));
}
