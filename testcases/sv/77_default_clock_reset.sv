module veryl_testcase_Module77 (
    input  var logic i_clk_a  ,
    input  var logic i_rst_a_n,
    input  var logic i_clk_b  ,
    input  var logic i_rst_b_n,
    input  var logic i_d      ,
    output var logic o_d  
);
    always_ff @ (posedge i_clk_a, negedge i_rst_a_n) begin
        if (!i_rst_a_n) begin
            o_d <= 0;
        end else begin
            o_d <= i_d;
        end
    end
endmodule
//# sourceMappingURL=../map/77_default_clock_reset.sv.map
