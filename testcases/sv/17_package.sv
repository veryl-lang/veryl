package veryl_testcase_Package17;
    // localparam declaration
    localparam int unsigned ParamX = 1;

    // struct declaration
    typedef struct packed {
        bit          [10-1:0] a  ;
        bit          [10-1:0] aa ;
        int unsigned          aaa;
    } A;

    // enum declaration
    typedef enum logic [2-1:0] {
        B_X = $bits(logic [2-1:0])'(1),
        B_Y = $bits(logic [2-1:0])'(2),
        B_Z
    } B;

    // function declaration
    function automatic logic [ParamX-1:0] FuncA(
        input  var logic [ParamX-1:0] a,
        output var logic [ParamX-1:0] b
    ) ;
        int unsigned c;
        c = 1;
        b = a + 1 + c;
        return a + 2;
    endfunction
endpackage
//# sourceMappingURL=../map/17_package.sv.map
