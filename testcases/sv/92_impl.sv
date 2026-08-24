package veryl_testcase_Package92;
    typedef struct packed {
        logic         sign;
        logic [5-1:0] exp ;
        logic [10-1:0] mts ;
    } __FpT__5__10;
    typedef struct packed {
        logic         sign;
        logic [8-1:0] exp ;
        logic [23-1:0] mts ;
    } __FpT__8__23;
    typedef struct packed {
        logic         sign;
        logic [2-1:0] exp ;
        logic [1-1:0] mts ;
    } __FpT__2__1;
    typedef struct packed {
        logic         sign;
        logic [3-1:0] exp ;
        logic [2-1:0] mts ;
    } __FpT__3__2;

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
    localparam int unsigned __FpT__2__1_BIAS = (1 << (2 - 1)) - 1;
    localparam int unsigned __FpT__3__2_BIAS = (1 << (3 - 1)) - 1;

    function automatic logic __FpT__2__1_is_nan(
        input var __FpT__2__1 self
    ) ;
        return 1'b0;
    endfunction

    typedef __FpT__5__10 Fp16;
    typedef __FpT__8__23 Fp32;
    typedef __FpT__2__1  Fp4 ;
    typedef __FpT__3__2  Fp6 ;

    function automatic logic __FpT__3__2_is_nan(
        input var __FpT__3__2 self
    ) ;
        return 1'b0;
    endfunction
endpackage

module veryl_testcase_Module92 (
    input  var veryl_testcase_Package92::Fp32 i_x  ,
    output var logic                          o_nan,
    output var int unsigned                   o_b  
);
    typedef struct packed {
        logic [8-1:0] a;
        logic [8-1:0] b;
    } PairT;

    

    PairT p;

    always_comb begin
        p.a = 8'd1;
        p.b = 8'd2;
    end

    logic        [8-1:0] _s; always_comb _s = PairT_sum(p);
    logic        [8-1:0] _t; always_comb _t = PairT_add(p, 8'd1);
    logic        [8-1:0] _u; always_comb _u = PairT_sum(p);
    int unsigned         _v; always_comb _v = PairT_WIDTH;
    int unsigned         _w; always_comb _w = veryl_testcase_Package92::__FpT__5__10_BIAS;

    always_comb o_nan = veryl_testcase_Package92::__FpT__8__23_is_nan(i_x);
    always_comb o_b   = veryl_testcase_Package92::__FpT__8__23_BIAS;

    veryl_testcase_Package92::Fp4 m ;
    logic                         _n; always_comb _n = veryl_testcase_Package92::__FpT__2__1_is_nan(m);
    int unsigned                  _o; always_comb _o = veryl_testcase_Package92::__FpT__2__1_BIAS;
    always_comb begin
        m.sign = 0;
        m.exp  = 0;
        m.mts  = 0;
    end

    veryl_testcase_Package92::Fp6 q ;
    logic                         _y; always_comb _y = veryl_testcase_Package92::__FpT__3__2_is_nan(q);
    int unsigned                  _z; always_comb _z = veryl_testcase_Package92::__FpT__3__2_BIAS;
    always_comb begin
        q.sign = 0;
        q.exp  = 0;
        q.mts  = 0;
    end

    initial begin
        PairT_log(p);
        PairT_log(p);
    end
endmodule
//# sourceMappingURL=../map/92_impl.sv.map
