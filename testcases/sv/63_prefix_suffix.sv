module veryl_testcase_Module63 (
    input logic i_clk  ,
    input logic i_rst_n
);
    veryl_testcase_Module63A u (
        .i_clk   (i_clk),
        .i_rst_n (i_rst_n)
    );

    logic a;

    always_ff @ (posedge i_clk, negedge i_rst_n) begin
        if (!i_rst_n) begin
            a <= 0;
        end else begin
            a <= 1;
        end
    end

    logic _b;
    always_comb _b = i_rst_n;
endmodule

module veryl_testcase_Module63A (
    input logic i_clk  ,
    input logic i_rst_n
);
endmodule
//# sourceMappingURL=../map/testcases/sv/63_prefix_suffix.sv.map
