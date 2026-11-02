

package veryl_testcase___Pkg98__8;
    typedef logic [8-1:0] Data;
endpackage


package veryl_testcase___Interface98____Pkg98__8__inline_types;
    typedef logic ack_ret_t;
    typedef logic zero_ack_ret_t;
    typedef veryl_testcase___Pkg98__8::Data get_data_ret_t;
endpackage
interface veryl_testcase___Interface98____Pkg98__8;
    logic                           ready;
    logic                           valid;
    veryl_testcase___Pkg98__8::Data data ;

    function automatic logic ack() ;
        return ready && valid;
    endfunction

    function automatic logic is_zero() ;
        return data == 0;
    endfunction

    function automatic logic zero_ack() ;
        return veryl_testcase___Interface98____Pkg98__8__inline_types::ack_ret_t'(ready && valid) && is_zero();
    endfunction

    function automatic veryl_testcase___Pkg98__8::Data get_data() ;
        return data;
    endfunction

    modport master (
        input  ready   ,
        output valid   ,
        output data    ,
        import ack     ,
        import is_zero ,
        import zero_ack,
        import get_data
    );

    modport slave (
        import ack     ,
        import is_zero ,
        import zero_ack,
        import get_data,
        output ready   ,
        input  valid   ,
        input  data    
    );

    logic _ack; always_comb _ack = veryl_testcase___Interface98____Pkg98__8__inline_types::ack_ret_t'(ready && valid);
endinterface

module veryl_testcase_Module98A (
    input var logic                                      i_clk    ,
    input var logic                                      i_rst_n_n,
    veryl_testcase___Interface98____Pkg98__8.slave slv_if   ,
    output var veryl_testcase___Pkg98__8::Data            o_data   
);
    always_comb slv_if.ready = 1;

    always_ff @ (posedge i_clk, negedge i_rst_n_n) begin
        if (!i_rst_n_n) begin
            o_data <= 0;
        end else if (veryl_testcase___Interface98____Pkg98__8__inline_types::zero_ack_ret_t'(veryl_testcase___Interface98____Pkg98__8__inline_types::ack_ret_t'(slv_if.ready
                && slv_if.valid) && slv_if.is_zero())) begin
            o_data <= 0;
        end else if (veryl_testcase___Interface98____Pkg98__8__inline_types::ack_ret_t'(slv_if.ready
            && slv_if.valid)) begin
            o_data <= veryl_testcase___Interface98____Pkg98__8__inline_types::get_data_ret_t'(slv_if.data);
        end
    end
endmodule

module veryl_testcase_Module98B (
    input  var logic                           i_clk        ,
    input  var logic                           i_rst_n_n    ,
    output var veryl_testcase___Pkg98__8::Data o_data    [2]
);
    veryl_testcase___Interface98____Pkg98__8 bus_if [2] ();

    for (genvar i = 0; i < 2; i++) begin :g
        always_comb bus_if[i].valid = 1;
        always_comb bus_if[i].data  = i;

        veryl_testcase_Module98A u (
            .i_clk     (i_clk    ),
            .i_rst_n_n (i_rst_n_n),
            .slv_if    (bus_if[i]),
            .o_data    (o_data[i])
        );
    end

    logic _ack; always_comb _ack = veryl_testcase___Interface98____Pkg98__8__inline_types::ack_ret_t'(bus_if[0].ready
        && bus_if[0].valid);
endmodule
//# sourceMappingURL=../map/98_inline_function.sv.map
