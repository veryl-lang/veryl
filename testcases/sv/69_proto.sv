


module veryl_testcase_Module69A #(
    parameter int unsigned A = 1,
    parameter int unsigned B = 1,
    parameter int unsigned C = 1
) (
    input  var logic a,
    input  var logic b,
    output var logic c
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
//# sourceMappingURL=../map/69_proto.sv.map
