module veryl_testcase_Module48;
endmodule

`ifdef __veryl_test_veryl_testcase_test1__
    `ifdef __veryl_wavedump_veryl_testcase_test1__
        module __veryl_wavedump;
            initial begin
                $dumpfile("test1.vcd");
                $dumpvars();
            end
        endmodule
    `endif

module test1;
   initial begin
       $display("hello");
       assert(0) else $info("info");
       assert(0) else $warning("warning");
       assert(0) else $error("error");
       assert(0) else $fatal(1, "fatal");
       $finish();
   end
endmodule
`endif

`ifdef __veryl_test_veryl_testcase_test2__
    `ifdef __veryl_wavedump_veryl_testcase_test2__
        module __veryl_wavedump;
            initial begin
                $dumpfile("test2.vcd");
                $dumpvars();
            end
        endmodule
    `endif

module test2;
    // parse error
    initial
endmodule
`endif

`ifdef __veryl_test_veryl_testcase_test3__
    `ifdef __veryl_wavedump_veryl_testcase_test3__
        module __veryl_wavedump;
            initial begin
                $dumpfile("test3.vcd");
                $dumpvars();
            end
        endmodule
    `endif

module test3;
    // elaborate error
    tri logic a;
    always_comb a = 1;
endmodule
`endif

`ifdef __veryl_test_veryl_testcase_test4__
    `ifdef __veryl_wavedump_veryl_testcase_test4__
        module __veryl_wavedump;
            initial begin
                $dumpfile("test4.vcd");
                $dumpvars();
            end
        endmodule
    `endif
module veryl_testcase_test4;
    initial begin
        $display("test4");
    end
endmodule
`endif
//# sourceMappingURL=../map/testcases/sv/48_test.sv.map
