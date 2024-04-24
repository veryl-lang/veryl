package veryl_testcase_Package50;
    typedef enum logic [1-1:0] {
        EnumA_memberA,
        EnumA_memberB
    } EnumA;
endpackage

module veryl_testcase_Module50;
    typedef veryl_testcase_Package50::EnumA EnumB;

    veryl_testcase_Package50::EnumA _a;
    always_comb _a = veryl_testcase_Package50::EnumA_memberA;
    EnumB                           _b;
    always_comb _b = veryl_testcase_Package50::EnumA_memberB;
endmodule
