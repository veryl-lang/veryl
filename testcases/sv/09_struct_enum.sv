module veryl_testcase_Module09;
    // struct declaration
    typedef struct packed {
        logic        [10-1:0] a  ;
        logic        [10-1:0] aa ;
        int unsigned          aaa;
    } A;

    // enum declaration
    typedef enum logic [2-1:0] {
        B_X = $bits(logic [2-1:0])'(1),
        B_Y = $bits(logic [2-1:0])'(2),
        B_Z
    } B;

    typedef enum logic [3-1:0] {
        C_X = 3'(2),
        C_Y = 3'(3),
        C_Z
    } C;

    typedef enum logic [1-1:0] {
        D_X
    } D;

    typedef enum logic [2-1:0] {
        E_X,
        E_Y,
        E_Z
    } E;

    typedef enum logic [3-1:0] {
        F_X = 3'd1,
        F_Y = 3'd2,
        F_Z = 3'd4
    } F;

    typedef enum logic [2-1:0] {
        G_X = 2'd0,
        G_Y = 2'd1,
        G_Z = 2'd3
    } G;

    typedef enum logic {
        FOO_H_0,
        FOO_H_1
    } H;

    A     a;
    B     b;
    C     c;
    D     d;
    E     e;
    F     f;
    G     g;
    H     h;
    logic i;

    always_comb a.a   = 1;
    always_comb a.aa  = 1;
    always_comb a.aaa = 1;
    always_comb b     = B_X;
    always_comb c     = C_X;
    always_comb d     = D_X;
    always_comb e     = E_X;
    always_comb f     = F_X;
    always_comb g     = G_X;
    always_comb h     = FOO_H_0;
    always_comb i     = a.a;
endmodule
//# sourceMappingURL=../map/09_struct_enum.sv.map
