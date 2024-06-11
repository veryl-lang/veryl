module veryl_testcase_Module02;
    // unsigned integer
    int unsigned     _a ;
    always_comb _a = 1;
    longint unsigned _aa;
    always_comb _aa = 1;

    // signed integer
    int signed     _b ;
    always_comb _b = 1;
    longint signed _bb;
    always_comb _bb = 1;

    // floating point
    shortreal _c ;
    always_comb _c = 1;
    real      _cc;
    always_comb _cc = 1;

    // 4 state (01xz) type
    logic                  _d  ;
    always_comb _d = 1;
    logic [10-1:0]         _dd ;
    always_comb _dd = 1;
    logic [10-1:0][10-1:0] _ddd;
    always_comb _ddd = 1;

    // 2 state (01) type
    bit                  _e  ;
    always_comb _e = 1;
    bit [10-1:0]         _ee ;
    always_comb _ee = 1;
    bit [10-1:0][10-1:0] _eee;
    always_comb _eee = 1;

    // array
    int unsigned     _f      [0:10-1];
    always_comb _f = 1;
    longint unsigned _ff     [0:10-1];
    always_comb _ff = 1;
    int signed       _fff    [0:10-1];
    always_comb _fff = 1;
    longint signed   _ffff   [0:10-1];
    always_comb _ffff = 1;
    shortreal        _fffff  [0:10-1];
    always_comb _fffff = 1;
    real             _ffffff [0:10-1];
    always_comb _ffffff = 1;
endmodule
//# sourceMappingURL=../map/testcases/sv/02_builtin_type.sv.map
