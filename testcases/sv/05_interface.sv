interface a #(
    parameter  int unsigned a    = 1,
    parameter  int unsigned aa   = 1,
    localparam int unsigned aaa  = 1
) ;
    parameter  int unsigned     a   = 1;
    localparam longint unsigned aa  = 1;

    logic                   a   ;
    logic  [10-1:0]         aa  ;
    bit    [10-1:0][10-1:0] aaa ;
    type_t aaaa [10-1:0]        ;

    modport a   (
        input  a  ,
        output aa ,
        inout  aaa
    );
endinterface
