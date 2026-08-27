package veryl_testcase_Package93;
    typedef struct packed {
        logic          sign;
        logic [5-1:0]  exp ;
        logic [10-1:0] mts ;
    } __FpT__5__10;
    typedef struct packed {
        logic          sign;
        logic [8-1:0]  exp ;
        logic [23-1:0] mts ;
    } __FpT__8__23;

    localparam int unsigned __FpT__5__10_BIAS = (1 << (5 - 1)) - 1;

    function automatic logic __FpT__5__10_is_nan(
        input var __FpT__5__10 self
    ) ;
        return (self.exp == '1) && (self.mts != '0);
    endfunction
    localparam int unsigned __FpT__8__23_BIAS = (1 << (8 - 1)) - 1;

    function automatic logic __FpT__8__23_is_nan(
        input var __FpT__8__23 self
    ) ;
        return (self.exp == '1) && (self.mts != '0);
    endfunction

    typedef __FpT__5__10 Fp16;
    typedef __FpT__8__23 Fp32;
endpackage

module veryl_testcase_Module93 (
    input  var veryl_testcase_Package93::Fp32 i_x  ,
    output var logic                          o_nan,
    output var int unsigned                   o_b  
);
    typedef struct packed {
        logic [8-1:0] a;
        logic [8-1:0] b;
    } PairT;

    localparam int unsigned PairT_WIDTH = 16;

    function automatic logic [8-1:0] PairT_sum(
        input var PairT self
    ) ;
        return self.a + self.b;
    endfunction

    function automatic logic [8-1:0] PairT_add(
        input var PairT self,
        input var logic [8-1:0] x
    ) ;
        return self.a + x;
    endfunction

    function automatic void PairT_log(
        input var PairT self
    ) ;
        $display("%d", self.a);
    endfunction

    PairT p;

    always_comb begin
        p.a = 8'd1;
        p.b = 8'd2;
    end

    logic        [8-1:0] _s; always_comb _s = PairT_sum(p);
    logic        [8-1:0] _t; always_comb _t = PairT_add(p, 8'd1);
    logic        [8-1:0] _u; always_comb _u = PairT_sum(p);
    int unsigned         _v; always_comb _v = PairT_WIDTH;
    int unsigned         _w; always_comb _w = veryl_testcase_Package93::__FpT__5__10_BIAS;

    always_comb o_nan = veryl_testcase_Package93::__FpT__8__23_is_nan(i_x);
    always_comb o_b   = veryl_testcase_Package93::__FpT__8__23_BIAS;

    initial begin
        PairT_log(p);
        PairT_log(p);
    end
endmodule
//# sourceMappingURL=../map/93_impl.sv.map
