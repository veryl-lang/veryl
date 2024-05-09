module veryl_testcase_Module54;
    function automatic logic [10-1:0] __FuncA__10(
        input logic [10-1:0] a
    ) ;
        return a + 1;
    endfunction
    function automatic logic [20-1:0] __FuncA__20(
        input logic [20-1:0] a
    ) ;
        return a + 1;
    endfunction

    logic [10-1:0] _a;
    always_comb _a = __FuncA__10(1);
    logic [20-1:0] _b;
    always_comb _b = __FuncA__20(1);
endmodule
