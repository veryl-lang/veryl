module veryl_testcase_Module56;
    import veryl_testcase_Package56A::X;
    veryl_testcase___Interface56A__Package56A_X u0 ();
    veryl_testcase___Interface56A__Package56B_X u1 ();
    veryl_testcase___Interface56B__Package56A_X u2 ();
    veryl_testcase___Interface56B__Package56A_X u3 ();
    veryl_testcase___Interface56B__3 u4 ();
    veryl_testcase___Interface56B__Package56A_X u5 ();

    logic _a;
    always_comb _a = u0._a;
    logic _b;
    always_comb _b = u2._b;
    logic _c;
    always_comb _c = u5._b;
endmodule

/// Generic interface test for doc comment
interface veryl_testcase___Interface56A__Package56A_X;
    logic [veryl_testcase_Package56A::X-1:0] _a;
endinterface

/// Generic interface test for doc comment
interface veryl_testcase___Interface56A__Package56B_X;
    logic [veryl_testcase_Package56B::X-1:0] _a;
endinterface

/// Generic interface test for doc comment
interface veryl_testcase___Interface56B__Package56A_X;
    logic [veryl_testcase_Package56A::X-1:0]                            _b;
endinterface
interface veryl_testcase___Interface56B__3;
    logic [3-1:0]                            _b;
endinterface

package veryl_testcase_Package56A;
    localparam int unsigned X = 1;
endpackage

package veryl_testcase_Package56B;
    localparam int unsigned X = 2;
endpackage
//# sourceMappingURL=../map/testcases/sv/56_generic_interface.sv.map
