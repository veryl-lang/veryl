// module declaration
module Module04 #(
    // module parameter
    parameter  int unsigned a   = 1,
    localparam int unsigned aa  = 1
) (
    // module port
    input  logic     [10-1:0] b    ,
    output logic     [10-1:0] bb   ,
    inout  tri logic [10-1:0] bbb  ,
    interface bbbb ,
    Interface05.d bbbbb 
) ;
    // localparam declaration
    localparam int unsigned     c   = 1;
    localparam longint unsigned cc  = 1;

    // variable declaration
    logic                  d  ;
    logic [10-1:0]         dd ;
    bit   [10-1:0][10-1:0] ddd;
endmodule
