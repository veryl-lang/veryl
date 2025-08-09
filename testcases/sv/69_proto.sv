


module veryl_testcase_Module69A #(
    parameter  int unsigned A       = 2        ,
    parameter  int unsigned B       = 3        ,
    parameter  int unsigned C       = 4        ,
    localparam int unsigned A_WIDTH = $clog2(A),
    localparam int unsigned B_WIDTH = $clog2(B),
    localparam int unsigned C_WIDTH = $clog2(C)
) (
    input  var logic                       [A_WIDTH-1:0] a,
    input  var logic                       [B_WIDTH-1:0] b,
    output var logic                       [C_WIDTH-1:0] c,
    veryl_testcase_Interface69.port               d
);
    always_comb c = a;
endmodule

package veryl_testcase_Package69A;
    typedef struct packed {
        logic a;
    } A;
endpackage


package veryl_testcase_Package69B;
    typedef veryl_testcase_Package69A::A A;
endpackage

interface veryl_testcase_Interface69;
    modport port (

    );
endinterface
//# sourceMappingURL=../map/69_proto.sv.map
