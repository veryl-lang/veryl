module veryl_testcase_Module37;
    int unsigned _a;
    assign _a = veryl_testcase_Package37::A;
    int unsigned _b;
    assign _b = veryl_testcase_Package37::B_C;
    int unsigned _c;
    assign _c = veryl_testcase_Package37::X();
endmodule
package veryl_testcase_Package37;
    localparam int unsigned A = 1;
    typedef 
    enum logic {
        B_C
    } B;

    function automatic int unsigned X;
        return 0;
    endfunction
endpackage
