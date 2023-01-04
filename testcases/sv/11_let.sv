module Module11 ;
    // variable declaration
    logic                  b  ;
    logic [10-1:0]         bb ;
    bit   [10-1:0][10-1:0] bbb;

    // variable declaration with assignment
    logic [10-1:0] c;
    assign c = 1;

    // assign declaration
    assign a    = 1;
    assign aa   = 1;
    assign aa.a = 1;
endmodule
