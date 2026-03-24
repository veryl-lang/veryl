module veryl_testcase_Module86A (
    input  var logic i_clk_a  ,
    input  var logic i_rst_a_n,
    input  var logic i_dat_a  ,
    output var logic o_dat_a  ,
    input  var logic i_clk_b  ,
    input  var logic i_rst_b_n,
    input  var logic i_dat_b  ,
    output var logic o_dat_b  
);
    // Inference from assign RHS
    logic x      ;
    always_comb x       = i_dat_a;
    always_comb o_dat_a = x;

    // Inference from always_ff clock
    logic y;
    always_ff @ (posedge i_clk_b, negedge i_rst_b_n) begin
        if (!i_rst_b_n) begin
            y <= 0;
        end else begin
            y <= i_dat_b;
        end
    end
    always_comb o_dat_b = y;
endmodule
//# sourceMappingURL=../map/86_clock_domain_inference.sv.map
