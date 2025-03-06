module veryl_testcase_Module61A (
    input  var logic i_dat,
    output var logic o_dat
);

    always_comb o_dat = i_dat;

endmodule

module veryl_testcase_Module61B (
    input  var logic i_clk,
    input  var logic i_dat,
    output var logic o_dat
);

    Synchronizer u_sync (
        .c (i_clk),
        .d (i_dat),
        .q (o_dat)
    );

endmodule
//# sourceMappingURL=../map/testcases/sv/61_unsafe_cdc.sv.map
