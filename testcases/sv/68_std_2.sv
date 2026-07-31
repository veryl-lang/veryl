module veryl_testcase_Module68H
    import veryl_testcase_Pkg68G::WIDTH;
    import __std___lzc_pkg__veryl_testcase_Pkg68G_WIDTH::lzc_result, __std___lzc_pkg__veryl_testcase_Pkg68G_WIDTH::lzc;
(
    input  var logic      [WIDTH-1:0] i_d     ,
    output var lzc_result             o_result
);



    always_comb o_result = lzc(i_d, 1'b1);
endmodule
//# sourceMappingURL=../map/68_std_2.sv.map
