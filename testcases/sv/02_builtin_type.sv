module veryl_testcase_Module02;
    // unsigned integer
    int unsigned     _a ; always_comb _a  = 1;
    longint unsigned _aa; always_comb _aa = 1;

    // signed integer
    int signed     _b ; always_comb _b  = 1;
    longint signed _bb; always_comb _bb = 1;

    // floating point
    shortreal _c ; always_comb _c  = 1;
    real      _cc; always_comb _cc = 1;

    // boolean
    logic _d ; always_comb _d  = 1'b1;
    logic _dd; always_comb _dd = 1'b0;

    // 4 state (01xz) type
    logic                  _e  ; always_comb _e   = 1;
    logic [10-1:0]         _ee ; always_comb _ee  = 1;
    logic [10-1:0][10-1:0] _eee; always_comb _eee = 1;

    // 2 state (01) type
    bit                  _f  ; always_comb _f   = 1;
    bit [10-1:0]         _ff ; always_comb _ff  = 1;
    bit [10-1:0][10-1:0] _fff; always_comb _fff = 1;

    // array
    int unsigned     _g      [0:2-1]; always_comb _g      = '{1, 1};
    longint unsigned _gg     [0:2-1]; always_comb _gg     = '{1, 1};
    int signed       _ggg    [0:2-1]; always_comb _ggg    = '{1, 1};
    longint signed   _gggg   [0:2-1]; always_comb _gggg   = '{1, 1};
    shortreal        _ggggg  [0:2-1]; always_comb _ggggg  = '{1, 1};
    real             _gggggg [0:2-1]; always_comb _gggggg = '{1, 1};
endmodule
//# sourceMappingURL=../map/02_builtin_type.sv.map
