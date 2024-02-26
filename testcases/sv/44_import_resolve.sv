module veryl_testcase_Module44;
    logic [10-1:0] a;
    logic [10-1:0] b;
    logic [10-1:0] c;

    import veryl_testcase_Package44A::z;
    import veryl_testcase_Package44B::*;

    assign a = veryl_testcase_Package44A::z;
    assign b = z;
    assign c = y;
endmodule

package veryl_testcase_Package44A;
    localparam int unsigned z = 0;
endpackage

package veryl_testcase_Package44B;
    localparam int unsigned y = 0;
endpackage
