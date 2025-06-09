module veryl_testcase_Module68A (
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

module veryl_testcase___Module68B__32 (
    __std___axi4_if____std___axi4_pkg__32__8__8__8__8__8__8__8.slave axi_if
);
    always_comb begin
        axi_if.awready = '0;
        axi_if.wready  = '0;
        axi_if.bvalid  = '0;
        axi_if.bresp   = __std_axi4_config::resp_variants_OKAY;
        axi_if.bid     = '0;
        axi_if.buser   = '0;
        axi_if.arready = '0;
        axi_if.rvalid  = '0;
        axi_if.rlast   = '0;
        axi_if.rdata   = '0;
        axi_if.rresp   = __std_axi4_config::resp_variants_OKAY;
        axi_if.rid     = '0;
        axi_if.ruser   = '0;
    end
endmodule

module veryl_testcase_Module68C;
    __std___axi4_if____std___axi4_pkg__32__8__8__8__8__8__8__8 axi_if ();
    always_comb begin
        axi_if.awvalid  = '0;
        axi_if.awaddr   = '0;
        axi_if.awsize   = __std_axi4_config::axsize_variants_BYTES_PER_TRANSFER_1;
        axi_if.awcache  = '0;
        axi_if.awburst  = __std_axi4_config::axburst_variants_FIXED_BURST;
        axi_if.awcache  = '0;
        axi_if.awprot   = '0;
        axi_if.awid     = '0;
        axi_if.awlen    = '0;
        axi_if.awlock   = '0;
        axi_if.awqos    = '0;
        axi_if.awregion = '0;
        axi_if.awuser   = '0;
        axi_if.wvalid   = '0;
        axi_if.wlast    = '0;
        axi_if.wdata    = '0;
        axi_if.wstrb    = '0;
        axi_if.wuser    = '0;
        axi_if.bready   = '0;
        axi_if.arvalid  = '0;
        axi_if.araddr   = '0;
        axi_if.arsize   = __std_axi4_config::axsize_variants_BYTES_PER_TRANSFER_1;
        axi_if.arcache  = '0;
        axi_if.arburst  = __std_axi4_config::axburst_variants_FIXED_BURST;
        axi_if.arcache  = '0;
        axi_if.arprot   = '0;
        axi_if.arid     = '0;
        axi_if.arlen    = '0;
        axi_if.arlock   = '0;
        axi_if.arqos    = '0;
        axi_if.arregion = '0;
        axi_if.aruser   = '0;
        axi_if.rready   = '0;
    end

    veryl_testcase___Module68B__32 u (
        .axi_if (axi_if)
    );
endmodule
//# sourceMappingURL=../map/68_std.sv.map
