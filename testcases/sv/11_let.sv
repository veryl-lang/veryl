module Module11 ;
    // variable declaration
    logic                  b   ;
    logic [10-1:0]         bb  ;
    bit   [10-1:0][10-1:0] _bbb;

    // variable declaration with assignment
    logic [10-1:0] _c;
    assign _c = 1;

    // assign declaration
    assign b    = 1;
    assign bb   = 1;
    assign aa.a = 1;
endmodule
