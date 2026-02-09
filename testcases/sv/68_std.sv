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

module veryl_testcase___Module68B____std___axi4_pkg__32__8__8__8__8__8__8__8____std___axi4_lite_pkg____std___axi4_pkg__32__8__8__8__8__8__8__8_ADDRESS_WIDTH____std___axi4_pkg__32__8__8__8__8__8__8__8_DATA_WIDTH_BYTES____std___axi4_pkg__32__8__8__8__8__8__8__8_ID_LENGTH____std___axi4_stream_pkg____std___axi4_pkg__32__8__8__8__8__8__8__8_DATA_WIDTH_BYTES__2____std___axi4_pkg__32__8__8__8__8__8__8__8_ID_LENGTH__5 (
    __std___axi4_if____std___axi4_pkg__32__8__8__8__8__8__8__8.slave                                                                                                                                                            axi4_if     ,
    __std___axi4_lite_if____std___axi4_lite_pkg____std___axi4_pkg__32__8__8__8__8__8__8__8_ADDRESS_WIDTH____std___axi4_pkg__32__8__8__8__8__8__8__8_DATA_WIDTH_BYTES____std___axi4_pkg__32__8__8__8__8__8__8__8_ID_LENGTH.slave axi4_lite_if,
    __std___axi4_stream_if____std___axi4_stream_pkg____std___axi4_pkg__32__8__8__8__8__8__8__8_DATA_WIDTH_BYTES__2____std___axi4_pkg__32__8__8__8__8__8__8__8_ID_LENGTH__5.receiver                                             axi4_stream_if
);
    always_comb begin
        axi4_if.awready = 0;
        axi4_if.wready  = 0;
        axi4_if.bvalid  = 0;
        axi4_if.bresp   = __std___axi4_pkg__32__8__8__8__8__8__8__8::resp_t'(0);
        axi4_if.bid     = __std___axi4_pkg__32__8__8__8__8__8__8__8::id_t'(0);
        axi4_if.buser   = __std___axi4_pkg__32__8__8__8__8__8__8__8::buser_t'(0);
        axi4_if.arready = 0;
        axi4_if.rvalid  = 0;
        axi4_if.rlast   = 0;
        axi4_if.rdata   = __std___axi4_pkg__32__8__8__8__8__8__8__8::data_t'(0);
        axi4_if.rresp   = __std___axi4_pkg__32__8__8__8__8__8__8__8::resp_t'(0);
        axi4_if.rid     = __std___axi4_pkg__32__8__8__8__8__8__8__8::id_t'(0);
        axi4_if.ruser   = __std___axi4_pkg__32__8__8__8__8__8__8__8::ruser_t'(0);
    end
    always_comb begin
        axi4_lite_if.bid     = __std___axi4_lite_pkg____std___axi4_pkg__32__8__8__8__8__8__8__8_ADDRESS_WIDTH____std___axi4_pkg__32__8__8__8__8__8__8__8_DATA_WIDTH_BYTES____std___axi4_pkg__32__8__8__8__8__8__8__8_ID_LENGTH::id_t'(0);
        axi4_lite_if.rid     = __std___axi4_lite_pkg____std___axi4_pkg__32__8__8__8__8__8__8__8_ADDRESS_WIDTH____std___axi4_pkg__32__8__8__8__8__8__8__8_DATA_WIDTH_BYTES____std___axi4_pkg__32__8__8__8__8__8__8__8_ID_LENGTH::id_t'(0);
        axi4_lite_if.awready = 0;
        axi4_lite_if.wready  = 0;
        axi4_lite_if.bvalid  = 0;
        axi4_lite_if.bresp   = __std___axi4_lite_pkg____std___axi4_pkg__32__8__8__8__8__8__8__8_ADDRESS_WIDTH____std___axi4_pkg__32__8__8__8__8__8__8__8_DATA_WIDTH_BYTES____std___axi4_pkg__32__8__8__8__8__8__8__8_ID_LENGTH::resp_t'(0);
        axi4_lite_if.arready = 0;
        axi4_lite_if.rvalid  = 0;
        axi4_lite_if.rdata   = __std___axi4_lite_pkg____std___axi4_pkg__32__8__8__8__8__8__8__8_ADDRESS_WIDTH____std___axi4_pkg__32__8__8__8__8__8__8__8_DATA_WIDTH_BYTES____std___axi4_pkg__32__8__8__8__8__8__8__8_ID_LENGTH::data_t'(0);
        axi4_lite_if.rresp   = __std___axi4_lite_pkg____std___axi4_pkg__32__8__8__8__8__8__8__8_ADDRESS_WIDTH____std___axi4_pkg__32__8__8__8__8__8__8__8_DATA_WIDTH_BYTES____std___axi4_pkg__32__8__8__8__8__8__8__8_ID_LENGTH::resp_t'(0);
    end
endmodule



module veryl_testcase_Module68C;
    __std___axi4_if____std___axi4_pkg__32__8__8__8__8__8__8__8                                                                                                                                                            axi4_if          ();
    __std___axi4_lite_if____std___axi4_lite_pkg____std___axi4_pkg__32__8__8__8__8__8__8__8_ADDRESS_WIDTH____std___axi4_pkg__32__8__8__8__8__8__8__8_DATA_WIDTH_BYTES____std___axi4_pkg__32__8__8__8__8__8__8__8_ID_LENGTH axi4_lite_if     ();
    __std___axi4_stream_if____std___axi4_stream_pkg____std___axi4_pkg__32__8__8__8__8__8__8__8_DATA_WIDTH_BYTES__2____std___axi4_pkg__32__8__8__8__8__8__8__8_ID_LENGTH__5                                                axi4_stream_if   ();
    always_comb begin
        axi4_if.awvalid  = 0;
        axi4_if.awaddr   = __std___axi4_pkg__32__8__8__8__8__8__8__8::addr_t'(0);
        axi4_if.awsize   = __std___axi4_pkg__32__8__8__8__8__8__8__8::size_t'(0);
        axi4_if.awburst  = __std___axi4_pkg__32__8__8__8__8__8__8__8::burst_t'(0);
        axi4_if.awcache  = __std___axi4_pkg__32__8__8__8__8__8__8__8::wcache_t'(0);
        axi4_if.awprot   = __std___axi4_pkg__32__8__8__8__8__8__8__8::proto_t'(0);
        axi4_if.awid     = __std___axi4_pkg__32__8__8__8__8__8__8__8::id_t'(0);
        axi4_if.awlen    = __std___axi4_pkg__32__8__8__8__8__8__8__8::num_bursts_t'(0);
        axi4_if.awlock   = __std___axi4_pkg__32__8__8__8__8__8__8__8::lock_t'(0);
        axi4_if.awqos    = __std___axi4_pkg__32__8__8__8__8__8__8__8::qos_t'(0);
        axi4_if.awregion = __std___axi4_pkg__32__8__8__8__8__8__8__8::region_t'(0);
        axi4_if.awuser   = __std___axi4_pkg__32__8__8__8__8__8__8__8::awuser_t'(0);
        axi4_if.wvalid   = 0;
        axi4_if.wlast    = 0;
        axi4_if.wdata    = __std___axi4_pkg__32__8__8__8__8__8__8__8::data_t'(0);
        axi4_if.wstrb    = __std___axi4_pkg__32__8__8__8__8__8__8__8::strb_t'(0);
        axi4_if.wuser    = __std___axi4_pkg__32__8__8__8__8__8__8__8::wuser_t'(0);
        axi4_if.bready   = 0;
        axi4_if.arvalid  = 0;
        axi4_if.araddr   = __std___axi4_pkg__32__8__8__8__8__8__8__8::addr_t'(0);
        axi4_if.arsize   = __std___axi4_pkg__32__8__8__8__8__8__8__8::size_t'(0);
        axi4_if.arburst  = __std___axi4_pkg__32__8__8__8__8__8__8__8::burst_t'(0);
        axi4_if.arcache  = __std___axi4_pkg__32__8__8__8__8__8__8__8::rcache_t'(0);
        axi4_if.arprot   = __std___axi4_pkg__32__8__8__8__8__8__8__8::proto_t'(0);
        axi4_if.arid     = __std___axi4_pkg__32__8__8__8__8__8__8__8::id_t'(0);
        axi4_if.arlen    = __std___axi4_pkg__32__8__8__8__8__8__8__8::num_bursts_t'(0);
        axi4_if.arlock   = __std___axi4_pkg__32__8__8__8__8__8__8__8::lock_t'(0);
        axi4_if.arqos    = __std___axi4_pkg__32__8__8__8__8__8__8__8::qos_t'(0);
        axi4_if.arregion = __std___axi4_pkg__32__8__8__8__8__8__8__8::region_t'(0);
        axi4_if.aruser   = __std___axi4_pkg__32__8__8__8__8__8__8__8::aruser_t'(0);
        axi4_if.rready   = 0;
    end
    always_comb begin
        axi4_lite_if.awvalid = 0;
        axi4_lite_if.awaddr  = __std___axi4_lite_pkg____std___axi4_pkg__32__8__8__8__8__8__8__8_ADDRESS_WIDTH____std___axi4_pkg__32__8__8__8__8__8__8__8_DATA_WIDTH_BYTES____std___axi4_pkg__32__8__8__8__8__8__8__8_ID_LENGTH::addr_t'(0);
        axi4_lite_if.awprot  = __std___axi4_lite_pkg____std___axi4_pkg__32__8__8__8__8__8__8__8_ADDRESS_WIDTH____std___axi4_pkg__32__8__8__8__8__8__8__8_DATA_WIDTH_BYTES____std___axi4_pkg__32__8__8__8__8__8__8__8_ID_LENGTH::proto_t'(0);
        axi4_lite_if.wvalid  = 0;
        axi4_lite_if.wdata   = __std___axi4_lite_pkg____std___axi4_pkg__32__8__8__8__8__8__8__8_ADDRESS_WIDTH____std___axi4_pkg__32__8__8__8__8__8__8__8_DATA_WIDTH_BYTES____std___axi4_pkg__32__8__8__8__8__8__8__8_ID_LENGTH::data_t'(0);
        axi4_lite_if.wstrb   = __std___axi4_lite_pkg____std___axi4_pkg__32__8__8__8__8__8__8__8_ADDRESS_WIDTH____std___axi4_pkg__32__8__8__8__8__8__8__8_DATA_WIDTH_BYTES____std___axi4_pkg__32__8__8__8__8__8__8__8_ID_LENGTH::strb_t'(0);
        axi4_lite_if.bready  = 0;
        axi4_lite_if.arvalid = 0;
        axi4_lite_if.araddr  = __std___axi4_lite_pkg____std___axi4_pkg__32__8__8__8__8__8__8__8_ADDRESS_WIDTH____std___axi4_pkg__32__8__8__8__8__8__8__8_DATA_WIDTH_BYTES____std___axi4_pkg__32__8__8__8__8__8__8__8_ID_LENGTH::addr_t'(0);
        axi4_lite_if.arprot  = __std___axi4_lite_pkg____std___axi4_pkg__32__8__8__8__8__8__8__8_ADDRESS_WIDTH____std___axi4_pkg__32__8__8__8__8__8__8__8_DATA_WIDTH_BYTES____std___axi4_pkg__32__8__8__8__8__8__8__8_ID_LENGTH::proto_t'(0);
        axi4_lite_if.rready  = 0;
    end
    always_comb begin
        axi4_stream_if.tvalid  = 0;
        axi4_stream_if.tready  = 0;
        axi4_stream_if.tdata   = __std___axi4_stream_pkg____std___axi4_pkg__32__8__8__8__8__8__8__8_DATA_WIDTH_BYTES__2____std___axi4_pkg__32__8__8__8__8__8__8__8_ID_LENGTH__5::data_t'(0);
        axi4_stream_if.tstrb   = __std___axi4_stream_pkg____std___axi4_pkg__32__8__8__8__8__8__8__8_DATA_WIDTH_BYTES__2____std___axi4_pkg__32__8__8__8__8__8__8__8_ID_LENGTH__5::strb_t'(0);
        axi4_stream_if.tkeep   = __std___axi4_stream_pkg____std___axi4_pkg__32__8__8__8__8__8__8__8_DATA_WIDTH_BYTES__2____std___axi4_pkg__32__8__8__8__8__8__8__8_ID_LENGTH__5::keep_t'(0);
        axi4_stream_if.tlast   = 0;
        axi4_stream_if.tid     = __std___axi4_stream_pkg____std___axi4_pkg__32__8__8__8__8__8__8__8_DATA_WIDTH_BYTES__2____std___axi4_pkg__32__8__8__8__8__8__8__8_ID_LENGTH__5::id_t'(0);
        axi4_stream_if.tdest   = __std___axi4_stream_pkg____std___axi4_pkg__32__8__8__8__8__8__8__8_DATA_WIDTH_BYTES__2____std___axi4_pkg__32__8__8__8__8__8__8__8_ID_LENGTH__5::dest_t'(0);
        axi4_stream_if.tuser   = __std___axi4_stream_pkg____std___axi4_pkg__32__8__8__8__8__8__8__8_DATA_WIDTH_BYTES__2____std___axi4_pkg__32__8__8__8__8__8__8__8_ID_LENGTH__5::user_t'(0);
        axi4_stream_if.twakeup = 0;
    end

    always_comb axi4_lite_if.awid = 0;
    always_comb axi4_lite_if.arid = 0;

    veryl_testcase___Module68B____std___axi4_pkg__32__8__8__8__8__8__8__8____std___axi4_lite_pkg____std___axi4_pkg__32__8__8__8__8__8__8__8_ADDRESS_WIDTH____std___axi4_pkg__32__8__8__8__8__8__8__8_DATA_WIDTH_BYTES____std___axi4_pkg__32__8__8__8__8__8__8__8_ID_LENGTH____std___axi4_stream_pkg____std___axi4_pkg__32__8__8__8__8__8__8__8_DATA_WIDTH_BYTES__2____std___axi4_pkg__32__8__8__8__8__8__8__8_ID_LENGTH__5 u (
        .axi4_if        (axi4_if       ),
        .axi4_lite_if   (axi4_lite_if  ),
        .axi4_stream_if (axi4_stream_if)
    );
endmodule
//# sourceMappingURL=../map/68_std.sv.map
