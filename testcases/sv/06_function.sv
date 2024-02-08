module veryl_testcase_Module06;
    localparam int unsigned ParamX = 1;

    // function without parameter
    function automatic logic [ParamX-1:0] FuncA(
        input  logic [ParamX-1:0] a,
        output logic [ParamX-1:0] b,
        ref    logic [ParamX-1:0] c
    ) ;
        int unsigned d;
        d = 1;
        b = a + 1 + d;
        c = a / 1;
        return a + 2;
    endfunction

    // function with parameter
    module FuncB #(
        parameter int unsigned ParamX = 1
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

    // void function
    function automatic void FuncC(
        input logic [ParamX-1:0] a,
        ref   logic [ParamX-1:0] c
    ) ;
        c = a / 1;
    endfunction

    logic [ParamX-1:0] a;
    logic [ParamX-1:0] b;
    logic [ParamX-1:0] c;
    logic [ParamX-1:0] d;

    // function call
    assign d = FuncA(a, b, c);

    // function call with parameter
    //assign a = FuncB #(ParamX: 1) (a, b, c);

    // void function call
    initial begin
        FuncC(a, c);
    end

    // system function call
    assign d = $clog2(a);
endmodule
