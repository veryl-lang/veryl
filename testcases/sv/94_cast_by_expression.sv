module veryl_testcase_Module94 #(
    parameter int unsigned W = 8 ,
    parameter int unsigned Q = 16
) (
    input  var logic [16-1:0] i_x,
    output var logic [32-1:0] o_a,
    output var logic [32-1:0] o_b,
    output var logic [32-1:0] o_c
);
    always_comb o_a = (W + 1)'(i_x);
    always_comb o_b = (Q + 1)'((i_x - 2));
    always_comb o_c = (W * 2 - 3)'(i_x);
endmodule
//# sourceMappingURL=../map/94_cast_by_expression.sv.map
