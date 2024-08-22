module veryl_testcase_Module09;
    // struct declaration
    typedef struct packed {
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

    typedef enum logic [3-1:0] {
        C_X = 2,
        C_Y = 3,
        C_Z
    } C;

    typedef enum logic {
        FOO_D_0,
        FOO_D_1
    } D;

    A     a;
    B     b;
    C     c;
    D     d;
    logic e;

    always_comb a.a   = 1;
    always_comb a.aa  = 1;
    always_comb a.aaa = 1;
    always_comb b     = B_X;
    always_comb c     = C_X;
    always_comb d     = FOO_D_0;
    always_comb e     = a.a;
endmodule
//# sourceMappingURL=../map/testcases/sv/09_struct_enum.sv.map
