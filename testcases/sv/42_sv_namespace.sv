module veryl_testcase_Module42 (
    input  var logic i_clk  ,
    input  var logic i_rst_n,
    input  var logic i_d    ,
    output var logic o_d
);
    localparam int unsigned a = pkg::paramA;
    //const b: u32 = pkg::paramA;

    delay u0 (
        .i_clk   (i_clk  ),
        .i_rst_n (i_rst_n),
        .i_d     (i_d    ),
        .o_d     (o_d    )
    );

    delay u1 (
        .i_clk   (i_clk  ),
        .i_rst_n (i_rst_n),
        .i_d     (i_d    ),
        .o_d     (o_d    )
    );
endmodule
//# sourceMappingURL=../map/42_sv_namespace.sv.map
