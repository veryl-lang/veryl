module veryl_testcase_Module14;
    logic aa ;
    logic bbb;

    // module instantiation
    veryl_testcase_Module14B x ();

    // module instantiation with parameter and port
    veryl_testcase_Module14C #(
        .a  (a  ),
        .aa (10 ),
        .aa (100)
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

module veryl_testcase_Module14C (
    input int unsigned a   ,
    input int unsigned bb  ,
    input int unsigned bbbb
);

endmodule

interface veryl_testcase_InterfaceA #(
    parameter int unsigned a = 1,
    parameter int unsigned b = 1
);

endinterface
