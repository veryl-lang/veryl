module veryl_testcase_Module14;
    localparam int unsigned X = 1;

    logic a  ;
    always_comb a = 1;
    logic aa ;
    always_comb aa = 1;
    logic bbb;
    always_comb bbb = 1;

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
    veryl_testcase_InterfaceA #(.a (a), .b (10)) yy  ();
    veryl_testcase_InterfaceA #(.a (a), .b (10)) xxx ();

    // interface array
    veryl_testcase_InterfaceA yyy [0:10-1] ();
endmodule

module veryl_testcase_Module14B;
endmodule

module veryl_testcase_Module14C #(
    parameter int unsigned X = 1,
    parameter int unsigned Y = 1
) (
    input logic [32-1:0] a   ,
    input logic [32-1:0] bb  ,
    input logic [32-1:0] bbbb
);
endmodule

interface veryl_testcase_InterfaceA #(
    parameter int unsigned a = 1,
    parameter int unsigned b = 1
);
endinterface
//# sourceMappingURL=../map/testcases/sv/14_inst.sv.map
