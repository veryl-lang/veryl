package veryl_testcase_Package17;
    // localparam declaration
    localparam int unsigned ParamX = 1;

    // variable declaration
    logic _b;

    // struct declaration
    typedef struct {
        logic        [10-1:0] a  ;
        logic        [10-1:0] aa ;
        int unsigned          aaa;
    } A;

    // enum declaration
    typedef enum logic [2-1:0] {
        B_X = 1,
        B_Y = 2,
        B_Z
    } B;

    // function declaration
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
endpackage
