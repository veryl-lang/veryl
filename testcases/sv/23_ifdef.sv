module veryl_testcase_Module23 #(
    `ifdef DEFINE_A
    `ifdef DEFINE_B
    `ifdef DEFINE_C
    `ifdef DEFINE_D
    parameter int unsigned ParamA = 1
    ,`endif
    `endif
    `endif
    `endif
    parameter int unsigned ParamB = 1
    `ifdef DEFINE_A
    ,
    parameter int unsigned ParamC = 1
    `endif
) (
    `ifdef DEFINE_A
    input logic port_a
    ,`endif
    input logic port_b

    `ifdef DEFINE_A
    ,
    input logic port_c
    `endif
);
    `ifdef DEFINE_A
    `ifdef DEFINE_B
    logic [10-1:0] _a;
    always_comb _a = 1;
    `endif
    `endif

    `ifdef DEFINE_A
    logic [10-1:0] _b;
    always_comb _b = 1;
    logic [10-1:0] _c;
    always_comb _c = 1;
    `endif

    logic _d;
    always_comb begin
        `ifdef DEFINE_D
        _d = 0;
        `endif

    end
endmodule

`ifdef DEFINE_A
module veryl_testcase_Module23_A;
endmodule
`endif

`ifndef DEFINE_A
module veryl_testcase_Module23_B;
endmodule
module veryl_testcase_Module23_C;
endmodule
`endif
//# sourceMappingURL=../map/testcases/sv/23_ifdef.sv.map
