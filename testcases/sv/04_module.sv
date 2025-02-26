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
    logic                  _d  ; always_comb _d   = 1;
    logic [10-1:0]         _dd ; always_comb _dd  = 1;
    bit   [10-1:0][10-1:0] _ddd; always_comb _ddd = 1;

    always_comb bb  = 0;
    assign bbb = 0;
endmodule

interface veryl_testcase_Interface04;
    logic c;

    modport d (
        input c
    );
endinterface
//# sourceMappingURL=../map/testcases/sv/04_module.sv.map
