package veryl_testcase___Package25__1;
    localparam int unsigned C = 1;
endpackage




module veryl_testcase_Module25
    import veryl_sample4___bar_pkg__32::*;
    import veryl_sample4___baz_pkg____Package25__1_C::*;
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
//# sourceMappingURL=../map/25_dependency.sv.map
