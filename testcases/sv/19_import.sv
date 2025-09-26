


package veryl_testcase_PackageA;
    import PackageA::A;
    import PackageA::*;
    localparam int unsigned A = 1;
endpackage

module veryl_testcase_Module19A;
    import PackageA::A;
    import PackageA::*;
    import veryl_testcase_PackageA::A;
    import veryl_testcase_PackageA::*;



endmodule

module veryl_testcase_Module19B
    import PackageA::A;
    import PackageA::*;
    import veryl_testcase_PackageA::A;
    import veryl_testcase_PackageA::*;
#(
    parameter int unsigned P = A
);



endmodule

module veryl_testcase_Module19C
    import PackageA::A;
    import PackageA::*;
    import veryl_testcase_PackageA::A;
    import veryl_testcase_PackageA::*;
(
    output var logic [A-1:0] o_d
);



    always_comb o_d = '0;
endmodule

module veryl_testcase_Module19D;
    import PackageA::A;
    import PackageA::*;
    if (1) begin :g0
        import veryl_testcase_PackageA::A;
        int unsigned _a; always_comb _a = A;
    end

    if (1'b1) begin :g1
        import veryl_testcase_PackageA::A;
        int unsigned _a; always_comb _a = A;
    end else begin :g1
        import veryl_testcase_PackageA::A;
        int unsigned _a; always_comb _a = A;
    end

    for (genvar i = 0; i < 1; i++) begin :g2
        import veryl_testcase_PackageA::A;
        int unsigned _a; always_comb _a = A;
    end
endmodule

interface veryl_testcase_Interface19A;
    import PackageA::A;
    import PackageA::*;
    import veryl_testcase_PackageA::A;
    import veryl_testcase_PackageA::*;



endinterface

interface veryl_testcase_Interface19B
    import PackageA::A;
    import PackageA::*;
    import veryl_testcase_PackageA::A;
    import veryl_testcase_PackageA::*;
#(
    parameter int unsigned P = A
);



endinterface

package veryl_testcase_Package19;
    import PackageA::A;
    import PackageA::*;
    import veryl_testcase_PackageA::A;
    import veryl_testcase_PackageA::*;



endpackage
//# sourceMappingURL=../map/19_import.sv.map
