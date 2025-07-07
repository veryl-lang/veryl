


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
