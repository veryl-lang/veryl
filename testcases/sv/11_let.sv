module veryl_testcase_Module11;
    // variable declaration
    logic                  b   ;
    logic [10-1:0]         bb  ;
    bit   [10-1:0][10-1:0] _bbb;

    // variable declaration with assignment
    logic [10-1:0] _c;
    always_comb _c = 1;

    // assign declaration
    always_comb b  = 1;
    always_comb bb = 1;
endmodule
