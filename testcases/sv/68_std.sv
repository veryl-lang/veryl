module veryl_testcase_Module68 (
    input  logic         i_clk  ,
    input  logic         i_rst_n,
    input  logic         i_push ,
    input  logic [8-1:0] i_data ,
    input  logic         i_pop  ,
    output logic [8-1:0] o_data 
);
    std_fifo u (
        .i_clk         (i_clk  ),
        .i_rst_n       (i_rst_n),
        .i_clear       ('0     ),
        .o_empty       (       ),
        .o_almost_full (       ),
        .o_full        (       ),
        .o_word_count  (       ),
        .i_push        (i_push ),
        .i_data        (i_data ),
        .i_pop         (i_pop  ),
        .o_data        (o_data )
    );
endmodule
//# sourceMappingURL=../map/testcases/sv/68_std.sv.map
