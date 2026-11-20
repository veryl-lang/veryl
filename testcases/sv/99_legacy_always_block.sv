module veryl_testcase_Module99 (
    input  var logic         i_clk  ,
    input  var logic         i_rst_n,
    input  var logic [8-1:0] i_a    ,
    output var logic [8-1:0] o_b    ,
    output var logic [8-1:0] o_c
);

    always @* begin
        o_b = i_a;
    end

    always @ (posedge i_clk, negedge i_rst_n) begin
        if (!i_rst_n) begin
            o_c <= 0;
        end else begin
            o_c <= i_a;
        end
    end
endmodule
//# sourceMappingURL=../map/99_legacy_always_block.sv.map
