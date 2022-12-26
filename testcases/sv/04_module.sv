// module declaration
module ModuleA #(
    // module parameter
    parameter  int unsigned a   = 1,
    localparam int unsigned aa  = 1
) (
    // module port
    input  logic [10-1:0] a  ,
    output logic [10-1:0] aa ,
    inout  logic [10-1:0] aaa
) ;
    // parameter declaration
    parameter  int unsigned     b   = 1;
    localparam longint unsigned bb  = 1;

    // variable declaration
    logic                  b  ;
    logic [10-1:0]         bb ;
    bit   [10-1:0][10-1:0] bbb;
endmodule
