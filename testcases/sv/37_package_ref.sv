package veryl_testcase_Package37;
    localparam int unsigned A = 1;

    typedef enum logic {
        B_C
    } B;

    function automatic int unsigned X;
        return 0;
    endfunction
endpackage

module veryl_testcase_Module37;
    int unsigned _a;
    always_comb _a = veryl_testcase_Package37::A;
    int unsigned _b;
    always_comb _b = veryl_testcase_Package37::B_C;
    int unsigned _c;
    always_comb _c = veryl_testcase_Package37::X();
endmodule
//# sourceMappingURL=../map/testcases/sv/37_package_ref.sv.map
