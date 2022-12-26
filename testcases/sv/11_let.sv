module ModuleA ;
    // variable declaration
    logic                  b  ;
    logic [10-1:0]         bb ;
    bit   [10-1:0][10-1:0] bbb;

    // variable declaration with assignment
    logic [10-1:0] c;
    assign c = 1;

    // assign declaration
    assign a   = 1;
    assign aa  = 1;
    assign aaa = 1;

    // module instantiation
    ModuleB x ();

    // module instantiation with parameter and port
    ModuleB #(
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

    // interface array
    InterfaceA yy [10-1:0] ();
endmodule
