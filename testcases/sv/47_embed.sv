package veryl_testcase_Package47A;
    localparam int unsigned A = 32;
endpackage

package veryl_testcase_Package47B;
    localparam int unsigned B = 64;
endpackage

module veryl_testcase_Module47A;
endmodule

module veryl_testcase___Module47B__Package47A_A;
    localparam int unsigned B = veryl_testcase_Package47A::A;
endmodule
module veryl_testcase___Module47B__Package47B_B;
    localparam int unsigned B = veryl_testcase_Package47B::B;
endmodule

module veryl_testcase_Module47C;
    veryl_testcase_Module47A u_a ();

    `ifndef SYNTHESIS
        `define bind_module_b \
        bind u_a \
        veryl_testcase___Module47B__Package47A_A u_b0 (); \
        bind u_a \
        veryl_testcase___Module47B__Package47B_B u_b1 ();

        `bind_module_b

        initial begin
            $display("hello");
        end
    `endif
endmodule

`ifndef SYNTHESIS
module test;
   initial begin
       $display("hello");
   end
endmodule
`endif

// comment
//# sourceMappingURL=../map/47_embed.sv.map
