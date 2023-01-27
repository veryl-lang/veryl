module veryl_testcase_Module02;
    // unsigned integer
    int unsigned     _a ;
    longint unsigned _aa;

    // signed integer
    int signed     _b ;
    longint signed _bb;

    // floating point
    shortreal _c ;
    real      _cc;

    // 4 state (01xz) type
    logic                  _d  ;
    logic [10-1:0]         _dd ;
    logic [10-1:0][10-1:0] _ddd;

    // 2 state (01) type
    bit                  _e  ;
    bit [10-1:0]         _ee ;
    bit [10-1:0][10-1:0] _eee;

    // array
    int unsigned     _f      [0:10-1];
    longint unsigned _ff     [0:10-1];
    int signed       _fff    [0:10-1];
    longint signed   _ffff   [0:10-1];
    shortreal        _fffff  [0:10-1];
    real             _ffffff [0:10-1];
endmodule
