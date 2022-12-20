// interface declaration
interface InterfaceA #(
    // interface parameter
    parameter  int unsigned a    = 1,
    parameter  int unsigned aa   = 1,
    localparam int unsigned aaa  = 1
) ;
    // parameter declaration
    parameter  int unsigned     a   = 1;
    localparam longint unsigned aa  = 1;

    // variable declaration
    logic                  a  ;
    logic [10-1:0]         aa ;
    bit   [10-1:0][10-1:0] aaa;

    // modport declaration
    modport a (
        input  a  ,
        output aa ,
        inout  aaa
    );
endinterface
