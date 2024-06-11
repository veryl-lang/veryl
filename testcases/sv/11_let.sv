module veryl_testcase_Module11;
    // variable declaration
    logic                  b   ;
    logic [10-1:0]         bb  ;
    bit   [10-1:0][10-1:0] _bbb;
    always_comb _bbb = 1;

    // variable declaration with assignment
    logic [10-1:0] _c;
    always_comb _c = 1;

    // assign declaration
    always_comb b  = 1;
    always_comb bb = 1;
endmodule
//# sourceMappingURL=../map/testcases/sv/11_let.sv.map
