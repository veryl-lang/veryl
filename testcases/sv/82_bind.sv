interface veryl_testcase_Interface82A;
    logic a;
    modport mp (
        input a
    );
endinterface

interface veryl_testcase_Interface82B;
    logic b;
    modport mp (
        input b
    );
endinterface

module veryl_testcase_Module82A (
    input var logic i_clk  ,
    input var logic i_rst_n
);
    veryl_testcase_Interface82A a ();
    veryl_testcase_Interface82B b ();
endmodule

module veryl_testcase_Module82B (
    veryl_testcase_Interface82A.mp a      ,
    input var logic                      b      ,
    input var logic                      i_clk  ,
    input var logic                      i_rst_n
);
endmodule

module veryl_testcase_Module82C;
    bind veryl_testcase_Module82A veryl_testcase_Module82B u0 (
        .a       (a      ),
        .b       (b.b    ),
        .i_clk   (i_clk  ),
        .i_rst_n (i_rst_n)
    );
endmodule

bind veryl_testcase_Module82A veryl_testcase_Module82B u1 (
    .a       (a      ),
    .b       (b.b    ),
    .i_clk   (i_clk  ),
    .i_rst_n (i_rst_n)
);
//# sourceMappingURL=../map/82_bind.sv.map
