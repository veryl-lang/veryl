module veryl_testcase_Module06;
    localparam int unsigned ParamX = 1;

    // function without parameter
    function automatic logic [ParamX-1:0] FuncA(
        input  var logic [ParamX-1:0] a,
        output var logic [ParamX-1:0] b
    ) ;
        int unsigned c;
        c = 1;
        b = a + 1 + c;
        return a + 2;
    endfunction

    // void function
    function automatic void FuncC(
        input  var logic [ParamX-1:0] a,
        output var logic [ParamX-1:0] b
    ) ;
        b = a / 1;
    endfunction

    logic [ParamX-1:0] a; always_comb a = 1;
    logic [ParamX-1:0] b;
    logic [ParamX-1:0] c;
    logic [ParamX-1:0] d;
    logic [ParamX-1:0] e;

    // function call
    always_comb c = FuncA(a, b);

    // void function call
    initial begin
        FuncC(a, d);
    end

    // system function call
    always_comb e = $clog2(a);
endmodule
//# sourceMappingURL=../map/testcases/sv/06_function.sv.map
