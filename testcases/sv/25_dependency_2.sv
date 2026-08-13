



module veryl_testcase_Module25A
    import veryl_sample4_qux_pkg;
    import veryl_sample4_bar_pkg;
    import veryl_sample4___bar_pkg__32::*;
    import veryl_sample4___baz_pkg__veryl_testcase___Package25__1_C__veryl_testcase___Package25__2_C::*;
(
    input var logic                    i_clk  ,
    input var logic                    i_rst_n,
    veryl_sample3_data_if.mp_in  in_if  ,
    veryl_sample3_data_if.mp_out out_if 
);



    veryl_sample3_data_if data_if ();

    veryl_sample_delay u0 (
        .i_clk     (i_clk       ),
        .i_rst_n_n (i_rst_n     ),
        .i_d       (in_if.data  ),
        .o_d       (data_if.data)
    );

    veryl_sample2_delay u1 (
        .i_clk     (i_clk       ),
        .i_rst_n_n (i_rst_n     ),
        .i_d       (data_if.data),
        .o_d       (out_if.data )
    );

    veryl_sample4___bar_module__veryl_sample4___foo_pkg__veryl_sample4___bar_pkg__32_BAR__veryl_sample4___bar_pkg__32 u2 ();
endmodule

module veryl_testcase___Module25B____Package25__1
    import veryl_sample4_qux_pkg;
    import veryl_sample4_bar_pkg;
    import veryl_testcase___Package25__1::*;
(
    veryl_sample4___qux_if__veryl_testcase___Package25__1_S.mp if3,
    veryl_sample4___qux_if__veryl_testcase___Package25__1_S.mp if4
);


    veryl_sample4___qux_if__veryl_testcase___Package25__1_S u5 ();
    veryl_sample4___qux_if__veryl_testcase___Package25__1_S u6 ();

    always_comb u5.qux.s = '0;
    always_comb u6.qux.s = '0;

    logic _a; always_comb _a = if3.qux.s;
    logic _b; always_comb _b = if4.qux.s;

    if (1) begin :g
        logic _c; always_comb _c = if3.qux.s;
        logic _d; always_comb _d = if4.qux.s;
        logic _e; always_comb _e = u5.qux.s;
        logic _f; always_comb _f = u6.qux.s;
    end
endmodule

module veryl_testcase___Module25C____Package25__1_C;
    import veryl_sample4_qux_pkg;
    import veryl_sample4_bar_pkg;
    veryl_sample4___quu_module__veryl_testcase___Package25__1_C u5 ();
endmodule


module veryl_testcase_Module25D;
    import veryl_sample4_qux_pkg;
    import veryl_sample4_bar_pkg;
    veryl_sample4___qux_if__veryl_testcase___Package25__1_S qux_if     ();
    always_comb qux_if.qux = 0;
    veryl_testcase___Module25B____Package25__1 u6 (
        .if3 (qux_if),
        .if4 (qux_if)
    );
    veryl_testcase___Module25C____Package25__1_C u7 ();

    logic _a; always_comb _a = qux_if.qux.s;
endmodule

module veryl_testcase_Module25E;
    import veryl_sample4_qux_pkg;
    import veryl_sample4_bar_pkg;
    localparam int unsigned QUX_0 = veryl_sample4_qux_pkg::QUX_0;
    localparam int unsigned QUX_1 = veryl_sample5_qux_pkg::QUX_1;
endmodule

module veryl_testcase_Module25F
    import veryl_sample4_qux_pkg;
    import veryl_sample4_bar_pkg;
#(
    parameter int unsigned WIDTH = 8
) (
    input  var logic [WIDTH-1:0] i_a,
    input  var logic [WIDTH-1:0] i_b,
    output var logic [WIDTH-1:0] o_c
);
    always_comb o_c = veryl_sample4___foo_func_1__WIDTH(i_a, i_b);

    function automatic int unsigned veryl_sample4_foo_func_2() ;
        return 2;
    endfunction
    function automatic logic [WIDTH-1:0] veryl_sample4___foo_func_1__WIDTH(
        input var logic [WIDTH-1:0] a,
        input var logic [WIDTH-1:0] b
    ) ;
        return a + b + veryl_sample4_foo_func_2();
    endfunction
endmodule

module veryl_testcase_Module25G;
    import veryl_sample4_qux_pkg;
    import veryl_sample4_bar_pkg;
    import veryl_sample4___baz_pkg__veryl_testcase___Package25__1_C__veryl_testcase___Package25__2_C::BAZ_0;
    import veryl_sample4___baz_pkg__veryl_testcase___Package25__1_C__veryl_testcase___Package25__2_C::BAZ_1;


    int unsigned _f0; always_comb _f0 = BAZ_0;
    int unsigned _f1; always_comb _f1 = BAZ_1;

    // the module import stays visible in nested scopes
    if (1) begin :g
        int unsigned _f2; always_comb _f2 = BAZ_0;
        if (1) begin :h
            int unsigned _f3; always_comb _f3 = BAZ_1;
        end
    end

    function automatic int unsigned f() ;
        return BAZ_0 + BAZ_1;
    endfunction
    int unsigned _f4; always_comb _f4 = f();
endmodule

package veryl_testcase_Pacakge25H;
    import veryl_sample4_qux_pkg;
    import veryl_sample4_bar_pkg;




endpackage

module veryl_testcase_Module25I;
    import veryl_sample4_qux_pkg;
    import veryl_sample4_bar_pkg;




    veryl_sample4___baz_if__veryl_sample4___baz_pkg__veryl_sample4___baz_pkg__1__2_BAZ_0__3 baz ();
    if (1) begin :g
        veryl_sample4___baz_module__veryl_sample4___baz_pkg__veryl_sample4___baz_pkg__1__2_BAZ_0__3 u (
            .baz (baz)
        );
    end
endmodule

module veryl_testcase_Module25J;
    import veryl_sample4_qux_pkg;
    import veryl_sample4_bar_pkg;




    veryl_sample4___baz_if__veryl_sample4___baz_pkg__veryl_sample4___baz_pkg__1__2_BAZ_0__3 baz ();
    // a guarded block still sees the module's unguarded import
    `ifdef DEFINE_A
    if (1) begin :g
        veryl_sample4___baz_module__veryl_sample4___baz_pkg__veryl_sample4___baz_pkg__1__2_BAZ_0__3 u (
            .baz (baz)
        );
    end
    `endif
endmodule

module veryl_testcase_Module25K;
    // the reverse: guarded import, unguarded reference
    import veryl_sample4_qux_pkg;
    import veryl_sample4_bar_pkg;

    `ifdef DEFINE_A

    `endif


    veryl_sample4___baz_if__veryl_sample4___baz_pkg__veryl_sample4___baz_pkg__1__2_BAZ_0__3 baz ();
    veryl_sample4___baz_module__veryl_sample4___baz_pkg__veryl_sample4___baz_pkg__1__2_BAZ_0__3 u (
        .baz (baz)
    );
endmodule

// Importing a package namespace itself makes the package name usable as a
// qualifier at the use site.
// https://github.com/veryl-lang/veryl/issues/3122



module veryl_testcase_Module25L;
    import veryl_sample4_qux_pkg;
    import veryl_sample4_bar_pkg;
    localparam int unsigned QUX_0 = veryl_sample4_qux_pkg::QUX_0;
    localparam int unsigned QUX_1 = veryl_sample4_qux_pkg::QUX_1;
endmodule

module veryl_testcase_Module25M;
    // Module-scope import of a package namespace.
    import veryl_sample4_qux_pkg;
    import veryl_sample4_bar_pkg;
    import veryl_sample4_qux_pkg;

    localparam int unsigned QUX_0 = veryl_sample4_qux_pkg::QUX_0;
endmodule

module veryl_testcase_Module25N;
    // A generic package is imported as-is and instantiated at the use site.
    import veryl_sample4_qux_pkg;
    import veryl_sample4_bar_pkg;
    localparam int unsigned BAR = veryl_sample4___bar_pkg__32::BAR;
endmodule

// Interfaces and modules can also be imported under their own names.
// https://github.com/veryl-lang/veryl/issues/1588

module veryl_testcase_Module25O
    import veryl_sample4_qux_pkg;
    import veryl_sample4_bar_pkg;
    import veryl_sample4_qux_if;
(
    veryl_sample4___qux_if__veryl_testcase___Package25__1_S.mp ifp
);
    // An interface is imported as-is; modport members stay qualified.

    veryl_sample4___qux_if__veryl_testcase___Package25__1_S u_if       ();
    always_comb u_if.qux.s = 0;
    logic _a        ; always_comb _a         = ifp.qux.s;
endmodule

module veryl_testcase_Module25P;
    // Modules are imported as-is and instantiated at the use site.
    import veryl_sample4_qux_pkg;
    import veryl_sample4_bar_pkg;
    import veryl_sample4_foo_module;
    import veryl_sample4_bar_module;


    veryl_sample4___foo_module__veryl_sample4___foo_pkg__veryl_sample4___bar_pkg__32_BAR                              u0 ();
    veryl_sample4___bar_module__veryl_sample4___foo_pkg__veryl_sample4___bar_pkg__32_BAR__veryl_sample4___bar_pkg__32 u1 ();
endmodule
//# sourceMappingURL=../map/25_dependency_2.sv.map
