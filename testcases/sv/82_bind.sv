interface veryl_testcase___Interface82A__32;
    logic [32-1:0] a;
    modport mp (
        input a
    );
endinterface

interface veryl_testcase___Interface82B__32;
    logic [32-1:0] b;
    modport mp (
        input b
    );
endinterface

module veryl_testcase___Module82A__32 (
    input var logic i_clk  ,
    input var logic i_rst_n
);
    veryl_testcase___Interface82A__32 a ();
    veryl_testcase___Interface82B__32 b ();
endmodule

module veryl_testcase___Module82B__32 (
    veryl_testcase___Interface82A__32.mp a      ,
    input var logic                            b      ,
    input var logic                            i_clk  ,
    input var logic                            i_rst_n
);
endmodule

module veryl_testcase_Module82C;
    bind veryl_testcase___Module82A__32 veryl_testcase___Module82B__32 u0 (
        .a       (a      ),
        .b       (b.b    ),
        .i_clk   (i_clk  ),
        .i_rst_n (i_rst_n)
    );
endmodule

bind veryl_testcase___Module82A__32 veryl_testcase___Module82B__32 u1 (
    .a       (a      ),
    .b       (b.b    ),
    .i_clk   (i_clk  ),
    .i_rst_n (i_rst_n)
);
//# sourceMappingURL=../map/82_bind.sv.map
