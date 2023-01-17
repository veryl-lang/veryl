module Module14;
    logic  aa ;
    logic  bbb;

    // module instantiation
    Module14B x ();

    // module instantiation with parameter and port
    Module14C #(
        .a  (a  ),
        .aa (10 ),
        .aa (100)
    ) xx (
        .a    (a  ),
        .bb   (aa ),
        .bbbb (bbb)
    );

    // interface instantiation
    InterfaceA y ();

    // interface instantiation with parameter
    InterfaceA #(.a (a), .b (10)) yy  ();
    InterfaceA #(.a (a), .b (10)) xxx ();

    // interface array
    InterfaceA yyy [10-1:0] ();
endmodule

module Module14B;


endmodule

module Module14C (
    input int unsigned a    ,
    input int unsigned bb   ,
    input int unsigned bbbb 
);


endmodule

interface InterfaceA #(
    parameter int unsigned a  = 1,
    parameter int unsigned b  = 1
);


endinterface
