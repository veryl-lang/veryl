module veryl_testcase_Module25 (
    input var logic                    i_clk  ,
    input var logic                    i_rst_n,
    veryl_sample3_data_if.mp_in  in_if  ,
    veryl_sample3_data_if.mp_out out_if 
);
    veryl_sample3_data_if data_if ();

    veryl_sample_delay u0 (
        .i_clk   (i_clk       ),
        .i_rst_n (i_rst_n     ),
        .i_d     (in_if.data  ),
        .o_d     (data_if.data)
    );

    veryl_sample2_delay u1 (
        .i_clk   (i_clk       ),
        .i_rst_n (i_rst_n     ),
        .i_d     (data_if.data),
        .o_d     (out_if.data )
    );
endmodule
//# sourceMappingURL=../map/25_dependency.sv.map
