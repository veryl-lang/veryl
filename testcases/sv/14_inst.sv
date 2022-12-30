module ModuleA ;
    // module instantiation
    ModuleB x ();

    // module instantiation with parameter and port
    ModuleC #(
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
module ModuleB ;

endmodule
module ModuleC (
    input int unsigned a    ,
    input int unsigned bb   ,
    input int unsigned bbbb 
) ;

endmodule
