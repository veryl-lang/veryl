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

    // void function
    function automatic void FuncC(
        input logic [ParamX-1:0] a,
        ref   logic [ParamX-1:0] c
    ) ;
        c = a / 1;
    endfunction

    logic [ParamX-1:0] a;
    always_comb a = 1;
    logic [ParamX-1:0] b;
    always_comb b = 1;
    logic [ParamX-1:0] c;
    always_comb c = 1;
    logic [ParamX-1:0] d;
    logic [ParamX-1:0] e;

    // function call
    always_comb d = FuncA(a, b, c);

    // void function call
    initial begin
        FuncC(a, c);
    end

    // system function call
    always_comb e = $clog2(a);
endmodule
