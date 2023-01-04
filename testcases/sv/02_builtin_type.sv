module Module02 ;
    // unsigned integer
    int unsigned     a  ;
    longint unsigned aa ;

    // signed integer
    int signed     b  ;
    longint signed bb ;

    // floating point
    shortreal c  ;
    real      cc ;

    // 4 state (01xz) type
    logic                  d  ;
    logic [10-1:0]         dd ;
    logic [10-1:0][10-1:0] ddd;

    // 2 state (01) type
    bit                  e  ;
    bit [10-1:0]         ee ;
    bit [10-1:0][10-1:0] eee;

    // array
    int unsigned     f      [10-1:0];
    longint unsigned ff     [10-1:0];
    int signed       fff    [10-1:0];
    longint signed   ffff   [10-1:0];
    shortreal        fffff  [10-1:0];
    real             ffffff [10-1:0];
endmodule
