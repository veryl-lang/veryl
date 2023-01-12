module Module11 ;
    // variable declaration
    logic                  _b  ;
    logic [10-1:0]         _bb ;
    bit   [10-1:0][10-1:0] _bbb;

    // variable declaration with assignment
    logic [10-1:0] _c;
    assign _c = 1;

    // assign declaration
    assign a    = 1;
    assign aa   = 1;
    assign aa.a = 1;
endmodule
