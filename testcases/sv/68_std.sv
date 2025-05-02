module veryl_testcase_Module68 (
    input  var logic         i_clk  ,
    input  var logic         i_rst_n,
    input  var logic         i_push ,
    input  var logic [8-1:0] i_data ,
    input  var logic         i_pop  ,
    output var logic [8-1:0] o_data 
);
    __std_fifo u (
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
//# sourceMappingURL=../map/68_std.sv.map
