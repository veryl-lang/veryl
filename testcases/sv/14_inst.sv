module veryl_testcase_Module14;
    localparam int unsigned A = 1;
    localparam int unsigned X = 1;

    logic a  ; always_comb a   = 1;
    logic aa ; always_comb aa  = 1;
    logic bbb; always_comb bbb = 1;

    // module instantiation
    veryl_testcase_Module14B x ();

    // module instantiation with parameter and port
    veryl_testcase_Module14C #(
        .X (X ),
        .Y (10)
    ) xx (
        .a    (a  ),
        .bb   (aa ),
        .bbbb (bbb)
    );

    // interface instantiation
    veryl_testcase_InterfaceA y ();

    // interface instantiation with parameter
    veryl_testcase_InterfaceA #( .A (A), .B (10) ) yy  ();
    veryl_testcase_InterfaceA #( .A (A), .B (10) ) xxx ();

    // interface array
    veryl_testcase_InterfaceA yyy [10] ();
endmodule

module veryl_testcase_Module14B;
endmodule

module veryl_testcase_Module14C #(
    parameter int unsigned X = 1,
    parameter int unsigned Y = 1
) (
    input var logic [32-1:0] a   ,
    input var logic [32-1:0] bb  ,
    input var logic [32-1:0] bbbb
);
endmodule

interface veryl_testcase_InterfaceA #(
    parameter int unsigned A = 1,
    parameter int unsigned B = 1
);
endinterface
//# sourceMappingURL=../map/14_inst.sv.map
