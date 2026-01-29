module veryl_testcase_Module02;
    // unsigned integer
    byte unsigned     _a   ; always_comb _a    = 1;
    shortint unsigned _aa  ; always_comb _aa   = 1;
    int unsigned      _aaa ; always_comb _aaa  = 1;
    longint unsigned  _aaaa; always_comb _aaaa = 1;

    // signed integer
    byte signed     _b   ; always_comb _b    = 1;
    shortint signed _bb  ; always_comb _bb   = 1;
    int signed      _bbb ; always_comb _bbb  = 1;
    longint signed  _bbbb; always_comb _bbbb = 1;

    // floating point
    shortreal _c ; always_comb _c  = 1;
    real      _cc; always_comb _cc = 1;

    // boolean
    bit   _d   ; always_comb _d    = 1'b1;
    bit   _dd  ; always_comb _dd   = 1'b0;
    logic _ddd ; always_comb _ddd  = 1'b1;
    logic _dddd; always_comb _dddd = 1'b0;

    // 4 state (01xz) type
    logic                  _e  ; always_comb _e   = 1;
    logic [10-1:0]         _ee ; always_comb _ee  = 1;
    logic [10-1:0][10-1:0] _eee; always_comb _eee = 1;

    // 2 state (01) type
    bit                  _f  ; always_comb _f   = 1;
    bit [10-1:0]         _ff ; always_comb _ff  = 1;
    bit [10-1:0][10-1:0] _fff; always_comb _fff = 1;

    // array
    byte unsigned     _g          [0:2-1]; always_comb _g          = '{1, 1};
    shortint unsigned _gg         [0:2-1]; always_comb _gg         = '{1, 1};
    int unsigned      _ggg        [0:2-1]; always_comb _ggg        = '{1, 1};
    longint unsigned  _gggg       [0:2-1]; always_comb _gggg       = '{1, 1};
    byte signed       _ggggg      [0:2-1]; always_comb _ggggg      = '{1, 1};
    shortint signed   _gggggg     [0:2-1]; always_comb _gggggg     = '{1, 1};
    int signed        _ggggggg    [0:2-1]; always_comb _ggggggg    = '{1, 1};
    longint signed    _gggggggg   [0:2-1]; always_comb _gggggggg   = '{1, 1};
    shortreal         _ggggggggg  [0:2-1]; always_comb _ggggggggg  = '{1, 1};
    real              _gggggggggg [0:2-1]; always_comb _gggggggggg = '{1, 1};
endmodule
//# sourceMappingURL=../map/02_builtin_type.sv.map
