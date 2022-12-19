module A ;
    // unsigned integer
    int unsigned     a ;
    longint unsigned a ;

    // signed integer
    int signed     a ;
    longint signed a ;

    // floating point
    real     a ;
    longreal a ;

    // 4 state (01xz) type
    logic                  a;
    logic [10-1:0]         a;
    logic [10-1:0][10-1:0] a;

    // 2 state (01) type
    bit                  a;
    bit [10-1:0]         a;
    bit [10-1:0][10-1:0] a;

    // array
    int unsigned     a [10-1:0];
    longint unsigned a [10-1:0];
    int signed       a [10-1:0];
    longint signed   a [10-1:0];
    real             a [10-1:0];
    longreal         a [10-1:0];
endmodule
