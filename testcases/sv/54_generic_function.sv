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
    logic [10-1:0] _b;
    always_comb _b = __FuncA__10(1);
    logic [20-1:0] _c;
    always_comb _c = __FuncA__20(1);
    logic [20-1:0] _d;
    always_comb _d = __FuncA__20(1);

    function automatic logic [10 + 2-1:0] __FuncB__10__2(
        input logic [10 + 2-1:0] a
    ) ;
        return a + 1;
    endfunction
    function automatic logic [10 + 4-1:0] __FuncB__10__4(
        input logic [10 + 4-1:0] a
    ) ;
        return a + 1;
    endfunction

    logic [12-1:0] _e;
    always_comb _e = __FuncB__10__2(1);
    logic [12-1:0] _f;
    always_comb _f = __FuncB__10__2(1);
    logic [14-1:0] _g;
    always_comb _g = __FuncB__10__4(1);
    logic [14-1:0] _h;
    always_comb _h = __FuncB__10__4(1);

    function automatic logic __FuncC__u() ;
        return u.a;
    endfunction

    veryl_testcase_Interface54 u ();

    logic _i;
    always_comb _i = __FuncC__u();
endmodule

interface veryl_testcase_Interface54;
    logic a;
endinterface
//# sourceMappingURL=../map/testcases/sv/54_generic_function.sv.map
