// module declaration
module veryl_testcase_Module04 #(
    // module parameter
    parameter  int unsigned a   = 1             ,
    localparam int unsigned aa  = 1             ,
    localparam type         aaa = logic [10-1:0]
) (
    // module port
    input  logic     [10-1:0] b    ,
    output logic     [10-1:0] bb   ,
    inout  tri logic [10-1:0] bbb  ,
    interface bbbb ,
    veryl_testcase_Interface04.d bbbbb
);
    // localparam declaration
    localparam int unsigned     c  = 1;
    localparam longint unsigned cc = 1;

    // variable declaration
    logic                  _d  ;
    logic [10-1:0]         _dd ;
    bit   [10-1:0][10-1:0] _ddd;
endmodule

interface veryl_testcase_Interface04;
    modport d (
        input c
    );
endinterface
