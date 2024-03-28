module veryl_testcase_Module25 (
    input  logic i_clk  ,
    input  logic i_rst_n,
    input  logic i_d    ,
    output logic o_d0   ,
    output logic o_d1   
);
    veryl_sample1_delay u0 (
        .i_clk   (i_clk  ),
        .i_rst_n (i_rst_n),
        .i_d     (i_d    ),
        .o_d     (o_d0   )
    );

    veryl_sample2_delay u1 (
        .i_clk   (i_clk  ),
        .i_rst_n (i_rst_n),
        .i_d     (i_d    ),
        .o_d     (o_d1   )
    );
endmodule
