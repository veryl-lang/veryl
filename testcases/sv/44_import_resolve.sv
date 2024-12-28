package veryl_testcase_Package44A;
    localparam int unsigned z = 0;
endpackage

package veryl_testcase_Package44B;
    localparam int unsigned y = 0;
endpackage

module veryl_testcase_Module44;
    logic [10-1:0] a;
    logic [10-1:0] b;
    logic [10-1:0] c;

    import veryl_testcase_Package44A::z;
    import veryl_testcase_Package44B::*;

    always_comb a = z;
    always_comb b = z;
    always_comb c = y;
endmodule
//# sourceMappingURL=../map/testcases/sv/44_import_resolve.sv.map
