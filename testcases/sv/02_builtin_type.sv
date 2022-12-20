module ModuleA ;
    // unsigned integer
    int unsigned     a  ;
    longint unsigned aa ;

    // signed integer
    int signed     a  ;
    longint signed aa ;

    // floating point
    shortreal a  ;
    real      aa ;

    // 4 state (01xz) type
    logic                  a  ;
    logic [10-1:0]         aa ;
    logic [10-1:0][10-1:0] aaa;

    // 2 state (01) type
    bit                  a  ;
    bit [10-1:0]         aa ;
    bit [10-1:0][10-1:0] aaa;

    // array
    int unsigned     a      [10-1:0];
    longint unsigned aa     [10-1:0];
    int signed       aaa    [10-1:0];
    longint signed   aaaa   [10-1:0];
    shortreal        aaaaa  [10-1:0];
    real             aaaaaa [10-1:0];
endmodule
