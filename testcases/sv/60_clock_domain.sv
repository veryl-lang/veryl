module veryl_testcase_Module60A (
    input  var logic i_clk_a  ,
    input  var logic i_rst_a_n,
    input  var logic i_dat_a  ,
    output var logic o_dat_a  ,
    input  var logic i_clk_b  ,
    input  var logic i_rst_b_n,
    input  var logic i_dat_b  ,
    output var logic o_dat_b  
);
    always_comb o_dat_a = i_dat_a;
    always_comb o_dat_b = i_dat_b;
endmodule

module veryl_testcase_Module60B (
    input  var logic i_clk   ,
    input  var logic i_clk_x2,
    input  var logic i_dat   ,
    output var logic o_dat   
);
    always_comb o_dat = i_dat;
endmodule

module veryl_testcase_Module60C (
    input  var logic i_clk,
    input  var logic i_dat,
    output var logic o_dat,
    input  var logic i_thr,
    output var logic o_thr
);
    always_comb o_dat = i_dat;
    always_comb o_thr = i_thr;
endmodule
//# sourceMappingURL=../map/60_clock_domain.sv.map
