module veryl_testcase_Module21;
    logic         a;
    logic [2-1:0] b;
    logic         c; always_comb c = 1;

    typedef enum logic {
        EnumA_A,
        EnumA_B
    } EnumA;

    typedef enum logic {
        EnumB_C,
        EnumB_D
    } EnumB;

    localparam type EnumC = EnumB;

    localparam int unsigned EnumD = 1;

    localparam logic [32-1:0] L = 2;

    always_comb a = EnumD'((EnumC'((EnumB'((EnumA'(c)))))));
    always_comb b = 2'(c);

    logic [2-1:0] _d; always_comb _d = L'(c);

    for (genvar i = 1; i < 4; i++) begin :g
        logic [2-1:0] _e; always_comb _e = i'(c);
    end
endmodule
//# sourceMappingURL=../map/21_cast.sv.map
