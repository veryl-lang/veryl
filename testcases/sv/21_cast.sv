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

    always_comb a = EnumB'((EnumA'(b)));
endmodule
//# sourceMappingURL=../map/testcases/sv/21_cast.sv.map
