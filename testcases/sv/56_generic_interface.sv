module veryl_testcase_Module56;
    veryl_testcase___Interface56A__Package56A u0 ();
    veryl_testcase___Interface56A__Package56B u1 ();
    veryl_testcase___Interface56B__Package56A u2 ();
    veryl_testcase___Interface56B__Package56A u4 ();
    veryl_testcase___Interface56B__Package56B u3 ();
endmodule

/// Generic interface test for doc comment
interface veryl_testcase___Interface56A__Package56A;
    logic [veryl_testcase_Package56A::X-1:0] _a;
endinterface

/// Generic interface test for doc comment
interface veryl_testcase___Interface56A__Package56B;
    logic [veryl_testcase_Package56B::X-1:0] _a;
endinterface

/// Generic interface test for doc comment
interface veryl_testcase___Interface56B__Package56A;
    logic [veryl_testcase_Package56A::X-1:0] _b;
endinterface
interface veryl_testcase___Interface56B__Package56B;
    logic [veryl_testcase_Package56B::X-1:0] _b;
endinterface

package veryl_testcase_Package56A;
    localparam int unsigned X = 1;
endpackage

package veryl_testcase_Package56B;
    localparam int unsigned X = 2;
endpackage
//# sourceMappingURL=../map/testcases/sv/56_generic_interface.sv.map
