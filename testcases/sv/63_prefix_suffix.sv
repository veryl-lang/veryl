module veryl_testcase_Module63 (
    input var logic i_clk   ,
    input var logic i_rst_n ,
    input var logic i_data_a
);
    veryl_testcase_Module63A u (
        .i_clk    (i_clk   ),
        .i_rst_n  (i_rst_n ),
        .i_data_a (i_data_a)
    );

    logic a;

    always_ff @ (posedge i_clk, negedge i_rst_n) begin
        if (!i_rst_n) begin
            a <= 0;
        end else begin
            a <= 1;
        end
    end

    logic _b; always_comb _b = i_rst_n;
endmodule

module veryl_testcase_Module63A (
    input var logic i_clk   ,
    input var logic i_rst_n ,
    input var logic i_data_a
);
endmodule
//# sourceMappingURL=../map/63_prefix_suffix.sv.map
