interface veryl_testcase_Interface06;
    logic a;
    logic b;

    modport mp (
        input a,
        input b
    );
endinterface

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

    // function with interface modport args
    function automatic logic [2-1:0] FuncD(
        input var logic __ab_if_a,
        input var logic __ab_if_b
    ) ;
        return {__ab_if_a, __ab_if_b};
    endfunction

    logic [ParamX-1:0] a; always_comb a = 1;
    logic [ParamX-1:0] b;
    logic [ParamX-1:0] c;
    logic [ParamX-1:0] d;
    logic [ParamX-1:0] e;
    logic [ParamX-1:0] f;
    logic [2-1:0]      g;

    // function call
    always_comb c = FuncA(a, b);

    // void function call
    initial begin
        FuncC(a, d);
    end

    // system function call
    always_comb e = $clog2(a);

    // function call with named args
    function automatic logic FuncB(
        input var logic aaa,
        input var logic bb 
    ) ;
        return aaa + bb;
    endfunction

    always_comb f = FuncB(
        .aaa (a + 11),
        .bb  (b + 2 )
    );

    veryl_testcase_Interface06 ab_if ();

    always_comb g = FuncD(ab_if.a, ab_if.b);
endmodule
//# sourceMappingURL=../map/06_function.sv.map
