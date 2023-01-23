module veryl_testcase_Module21;
    logic  a;
    logic  b;
    typedef 
    enum logic  {
        EnumA_A,
        EnumA_B
    } EnumA;
    typedef 
    enum logic  {
        EnumB_C,
        EnumB_D
    } EnumB;

    assign a = EnumA'(EnumB'(b));
endmodule
