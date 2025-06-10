module veryl_testcase_Module47;
    `ifndef SYNTHESIS
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
