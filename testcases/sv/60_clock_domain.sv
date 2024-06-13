module veryl_testcase_Module60A (
    input  logic i_clk_a  ,
    input  logic i_rst_a_n,
    input  logic i_dat_a  ,
    output logic o_dat_a  ,
    input  logic i_clk_b  ,
    input  logic i_rst_b_n,
    input  logic i_dat_b  ,
    output logic o_dat_b  
);
    always_comb o_dat_a = i_dat_a;
    always_comb o_dat_b = i_dat_b;
endmodule

module veryl_testcase_Module60B (
    input  logic i_clk   ,
    input  logic i_clk_x2,
    input  logic i_dat   ,
    output logic o_dat   
);
    always_comb o_dat = i_dat;
endmodule

module veryl_testcase_Module60C (
    input  logic i_clk,
    input  logic i_dat,
    output logic o_dat,
    input  logic i_thr,
    output logic o_thr
);
    always_comb o_dat = i_dat;
    always_comb o_thr = i_thr;
endmodule
//# sourceMappingURL=../map/testcases/sv/60_clock_domain.sv.map
