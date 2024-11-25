module veryl_testcase_Module21;
    logic         a;
    logic [2-1:0] b;
    logic         c;
    always_comb c = 1;

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

    always_comb a = EnumD'((EnumC'((EnumB'((EnumA'(c)))))));
    always_comb b = 2'(c);
endmodule
//# sourceMappingURL=../map/testcases/sv/21_cast.sv.map
