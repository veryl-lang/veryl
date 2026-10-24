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

interface veryl_testcase_Interface60D;
    logic dat;
endinterface

module veryl_testcase_Module60D (
    input  var logic i_clk_hclk  ,
    input  var logic i_rst_hclk_n,
    input  var logic i_dat_hclk  ,
    output var logic o_dat_hclk  ,
    input  var logic i_dat_xclk  ,
    output var logic o_dat_xclk  ,
    input  var logic i_dat_data  ,
    output var logic o_dat_data  
);
    logic r_dat;
    logic w_dat; always_comb w_dat = i_dat_xclk;

    veryl_testcase_Interface60D u_if ();

    always_ff @ (posedge i_clk_hclk, negedge i_rst_hclk_n) begin
        if (!i_rst_hclk_n) begin
            r_dat <= 0;
        end else begin
            r_dat <= i_dat_hclk;
        end
    end

    always_comb o_dat_hclk = r_dat;
    always_comb o_dat_xclk = w_dat;
    always_comb u_if.dat   = i_dat_data;
    always_comb o_dat_data = u_if.dat;
endmodule
//# sourceMappingURL=../map/60_clock_domain.sv.map
