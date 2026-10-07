module veryl_testcase_Module96 (
    input  var logic [`WIDTH-1:0] i_a,
    output var logic [`WIDTH-1:0] o_a
);
    localparam int unsigned A = `A;
    localparam int unsigned B = `PKG::B;

    `TYPE c;

    always_comb c   = A + B;
    always_comb o_a = i_a;
endmodule
//# sourceMappingURL=../map/96_sv_define.sv.map
