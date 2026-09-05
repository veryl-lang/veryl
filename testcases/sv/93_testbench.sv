module veryl_testcase_Module93;
    logic inner;
    always_comb inner = 1;
endmodule

module veryl_testcase_Module93Tb;
    veryl_testcase_Module93 dut ();

    logic seen;

    initial begin
        seen = dut.inner;
    end
endmodule
//# sourceMappingURL=../map/93_testbench.sv.map
