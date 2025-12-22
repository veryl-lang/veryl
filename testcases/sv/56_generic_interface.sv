package veryl_testcase_Package56A;
    localparam int unsigned X = 1;
endpackage

package veryl_testcase_Package56B;
    localparam int unsigned X = 2;
endpackage


package veryl_testcase_Package56C;
    typedef StructA A;

    typedef struct packed {
        logic a;
        logic b;
    } StructA;
endpackage

module veryl_testcase_Module56
    import veryl_testcase_Package56A::X;
(
    veryl_testcase___Interface56D__Package56C_StructA.X x0,
    veryl_testcase___Interface56E__Package56C.X         x1
);


    veryl_testcase___Interface56A__Package56A_X u_a0 ();
    veryl_testcase___Interface56A__Package56B_X u_a1 ();
    veryl_testcase___Interface56B__Package56A_X u_b0 ();
    veryl_testcase___Interface56B__Package56A_X u_b1 ();

    veryl_testcase___Interface56B__3            u_b2 ();
    veryl_testcase___Interface56B__Package56A_X u_b3 ();
    veryl_testcase___Module56A__1               u_a2 ();

    veryl_testcase___Module56C____Interface56C__2 u_c0 ();

    veryl_testcase___Interface56D__Package56C_StructA u_d0 ();
    veryl_testcase___Interface56E__Package56C         u_d1 ();

    always_comb u_a0._a   = 0;
    always_comb u_a1._a   = 0;
    always_comb u_b0._b   = 0;
    always_comb u_b1._b   = 0;
    always_comb u_b2._b   = 0;
    always_comb u_b3._b   = 0;
    always_comb u_d0._b.a = 0;
    always_comb u_d0._b.b = 0;
    always_comb u_d1._b.a = 0;
    always_comb u_d1._b.b = 0;

    logic _a; always_comb _a = u_a0._a;
    logic _b; always_comb _b = u_b0._b;
    logic _c; always_comb _c = u_b3._b;
    logic _d; always_comb _d = x0._b.a;
    logic _e; always_comb _e = x1._b.a;
    logic _f; always_comb _f = u_d0._b.a;
    logic _g; always_comb _g = u_d1._b.a;
endmodule

module veryl_testcase___Module56A__1;
    veryl_testcase___Interface56A__1 u ();
    function automatic void f() ;
    endfunction
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
interface veryl_testcase___Interface56A__1;
    logic [1-1:0] _a;
endinterface

/// Generic interface test for doc comment
interface veryl_testcase___Interface56B__Package56A_X;
    logic [veryl_testcase_Package56A::X-1:0] _b;
endinterface
interface veryl_testcase___Interface56B__3;
    logic [3-1:0] _b;
endinterface


interface veryl_testcase___Interface56C__2;

    localparam int unsigned C_WIDTH = 2;

    logic [C_WIDTH-1:0] _c;
endinterface

module veryl_testcase___Module56C____Interface56C__2;
    veryl_testcase___Interface56C__2 u ();
endmodule

interface veryl_testcase___Interface56D__Package56C_StructA;
    veryl_testcase_Package56C::StructA _b;

    modport X (
        input _b
    );
endinterface

interface veryl_testcase___Interface56E__Package56C;
    veryl_testcase_Package56C::A _b;

    modport X (
        input _b
    );
endinterface
//# sourceMappingURL=../map/56_generic_interface.sv.map
