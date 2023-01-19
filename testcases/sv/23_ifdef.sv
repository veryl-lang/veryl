module Module23 #(
    `ifdef DEFINE_A
    parameter int unsigned ParamA  = 1
    ,`endif
    parameter int unsigned ParamB  = 1
    `ifdef DEFINE_A
    ,

    parameter int unsigned ParamC  = 1
    `endif
) (
    `ifdef DEFINE_A
    input logic  port_a
    ,`endif
    input logic  port_b
    `ifdef DEFINE_A
    ,

    input logic  port_c
    `endif
);
    `ifdef DEFINE_A
    logic [10-1:0] _a;
    `endif
    `ifdef DEFINE_A

    logic [10-1:0] _b;
    logic [10-1:0] _c;
    `endif

endmodule
`ifdef DEFINE_A
module Module23_A;

endmodule
`endif
`ifndef DEFINE_A

module Module23_B;

endmodule
module Module23_C;

endmodule
`endif
