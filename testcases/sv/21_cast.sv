module veryl_testcase_Module21;
    logic a;
    logic b;
    always_comb b = 1;

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

    always_comb a = EnumD'((EnumC'((EnumB'((EnumA'(b)))))));
endmodule
//# sourceMappingURL=../map/testcases/sv/21_cast.sv.map
