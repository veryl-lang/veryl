module ModuleA ;
    // function without parameter
    function automatic logic [ParamX-1:0] FuncA(
        input  logic [ParamX-1:0] a,
        output logic [ParamX-1:0] b,
        ref    logic [ParamX-1:0] c
    ) ;
        int unsigned d ;
        d = 1;
        b = a + 1 + d;
        c = a / 1;
        return a + 2;
    endfunction

    // function with parameter
    module FuncB #(
        parameter  int unsigned ParamX  = 1
    );
        function automatic logic [ParamX-1:0] FuncB(
            input  logic [ParamX-1:0] a,
            output logic [ParamX-1:0] b,
            ref    logic [ParamX-1:0] c
        ) ;
            b = a + 1;
            c = a / 1;
            return a + 2;
        endfunction
    endmodule

    // function call
    assign a = FuncA(a, b, c);

    // function call with parameter
    //assign a = FuncB #(ParamX: 1) (a, b, c);

    // system function call
    assign a = $clog2(a);
endmodule
