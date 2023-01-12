module Module02 ;
    // unsigned integer
    int unsigned     _a  ;
    longint unsigned _aa ;

    // signed integer
    int signed     _b  ;
    longint signed _bb ;

    // floating point
    shortreal _c  ;
    real      _cc ;

    // 4 state (01xz) type
    logic                  _d  ;
    logic [10-1:0]         _dd ;
    logic [10-1:0][10-1:0] _ddd;

    // 2 state (01) type
    bit                  _e  ;
    bit [10-1:0]         _ee ;
    bit [10-1:0][10-1:0] _eee;

    // array
    int unsigned     _f      [10-1:0];
    longint unsigned _ff     [10-1:0];
    int signed       _fff    [10-1:0];
    longint signed   _ffff   [10-1:0];
    shortreal        _fffff  [10-1:0];
    real             _ffffff [10-1:0];
endmodule
