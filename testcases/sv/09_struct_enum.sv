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

    typedef enum logic {
        FOO_C_0,
        FOO_C_1
    } C;

    A a;
    B b;
    C c;

    always_comb a.a   = 1;
    always_comb a.aa  = 1;
    always_comb a.aaa = 1;
    always_comb b     = B_X;
    always_comb c     = FOO_C_0;
endmodule
