



module veryl_testcase_Module25A
    import veryl_sample4___bar_pkg__32::*;
    import veryl_sample4___baz_pkg__veryl_testcase___Package25__1_C::*;
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

module veryl_testcase_Module25C;
    veryl_sample4___qux_if__veryl_testcase___Package25__1_S qux_if ();
    veryl_testcase___Module25B____Package25__1 u5 (
        .if3 (qux_if),
        .if4 (qux_if)
    );
endmodule
//# sourceMappingURL=../map/25_dependency_2.sv.map
