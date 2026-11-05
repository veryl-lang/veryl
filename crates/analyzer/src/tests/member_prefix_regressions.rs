use super::layout_regressions::evaluate_contract_constant;

#[test]
fn aggregate_contract_called_member_index_stays_within_member() {
    let code = "module Top{struct Leaf{hi:logic<2>,lo:logic<3>,}struct Packet{head:logic<4>,body:Leaf<2>,tail:logic<4>,}function next(i:input logic<4>)->logic<4>{return i+1;}function probe(i:input logic<4>)->logic<2>{var storage:Packet;storage=18'h3ffff;return storage.body[next(i)].hi;}const RESULT:logic<2> = probe(1);}";
    let got = evaluate_contract_constant(code);
    assert_eq!((got.payload_u128(), got.mask_xz_u128()), (0, 3));
}
