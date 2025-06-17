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
        axi_if.awready = 0;
        axi_if.wready  = 0;
        axi_if.bvalid  = 0;
        axi_if.bresp   = __std___axi4_pkg__32__8__8__8__8__8__8__8::resp_t'(0);
        axi_if.bid     = __std___axi4_pkg__32__8__8__8__8__8__8__8::id_t'(0);
        axi_if.buser   = __std___axi4_pkg__32__8__8__8__8__8__8__8::buser_t'(0);
        axi_if.arready = 0;
        axi_if.rvalid  = 0;
        axi_if.rlast   = 0;
        axi_if.rdata   = __std___axi4_pkg__32__8__8__8__8__8__8__8::data_t'(0);
        axi_if.rresp   = __std___axi4_pkg__32__8__8__8__8__8__8__8::resp_t'(0);
        axi_if.rid     = __std___axi4_pkg__32__8__8__8__8__8__8__8::id_t'(0);
        axi_if.ruser   = __std___axi4_pkg__32__8__8__8__8__8__8__8::ruser_t'(0);
    end
endmodule

module veryl_testcase_Module68C;
    __std___axi4_if____std___axi4_pkg__32__8__8__8__8__8__8__8 axi_if          ();
    always_comb begin
        axi_if.awvalid  = 0;
        axi_if.awaddr   = __std___axi4_pkg__32__8__8__8__8__8__8__8::addr_t'(0);
        axi_if.awsize   = __std___axi4_pkg__32__8__8__8__8__8__8__8::size_t'(0);
        axi_if.awburst  = __std___axi4_pkg__32__8__8__8__8__8__8__8::burst_t'(0);
        axi_if.awcache  = __std___axi4_pkg__32__8__8__8__8__8__8__8::wcache_t'(0);
        axi_if.awprot   = __std___axi4_pkg__32__8__8__8__8__8__8__8::proto_t'(0);
        axi_if.awid     = __std___axi4_pkg__32__8__8__8__8__8__8__8::id_t'(0);
        axi_if.awlen    = __std___axi4_pkg__32__8__8__8__8__8__8__8::num_bursts_t'(0);
        axi_if.awlock   = __std___axi4_pkg__32__8__8__8__8__8__8__8::lock_t'(0);
        axi_if.awqos    = __std___axi4_pkg__32__8__8__8__8__8__8__8::qos_t'(0);
        axi_if.awregion = __std___axi4_pkg__32__8__8__8__8__8__8__8::region_t'(0);
        axi_if.awuser   = __std___axi4_pkg__32__8__8__8__8__8__8__8::awuser_t'(0);
        axi_if.wvalid   = 0;
        axi_if.wlast    = 0;
        axi_if.wdata    = __std___axi4_pkg__32__8__8__8__8__8__8__8::data_t'(0);
        axi_if.wstrb    = __std___axi4_pkg__32__8__8__8__8__8__8__8::strb_t'(0);
        axi_if.wuser    = __std___axi4_pkg__32__8__8__8__8__8__8__8::wuser_t'(0);
        axi_if.bready   = 0;
        axi_if.arvalid  = 0;
        axi_if.araddr   = __std___axi4_pkg__32__8__8__8__8__8__8__8::addr_t'(0);
        axi_if.arsize   = __std___axi4_pkg__32__8__8__8__8__8__8__8::size_t'(0);
        axi_if.arburst  = __std___axi4_pkg__32__8__8__8__8__8__8__8::burst_t'(0);
        axi_if.arcache  = __std___axi4_pkg__32__8__8__8__8__8__8__8::rcache_t'(0);
        axi_if.arprot   = __std___axi4_pkg__32__8__8__8__8__8__8__8::proto_t'(0);
        axi_if.arid     = __std___axi4_pkg__32__8__8__8__8__8__8__8::id_t'(0);
        axi_if.arlen    = __std___axi4_pkg__32__8__8__8__8__8__8__8::num_bursts_t'(0);
        axi_if.arlock   = __std___axi4_pkg__32__8__8__8__8__8__8__8::lock_t'(0);
        axi_if.arqos    = __std___axi4_pkg__32__8__8__8__8__8__8__8::qos_t'(0);
        axi_if.arregion = __std___axi4_pkg__32__8__8__8__8__8__8__8::region_t'(0);
        axi_if.aruser   = __std___axi4_pkg__32__8__8__8__8__8__8__8::aruser_t'(0);
        axi_if.rready   = 0;
    end

    veryl_testcase___Module68B__32 u (
        .axi_if (axi_if)
    );
endmodule
//# sourceMappingURL=../map/68_std.sv.map
