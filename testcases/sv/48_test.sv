module veryl_testcase_Module48;
endmodule

`ifdef __veryl_test_veryl_testcase_test1__

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

module test2;
    // parse error
    initial
endmodule
`endif

`ifdef __veryl_test_veryl_testcase_test3__

module test3;
    // elaborate error
    tri logic a;
    always_comb a = 1;
endmodule
`endif

`ifdef __veryl_test_veryl_testcase_test4__
module veryl_testcase_test4;
    initial begin
        $display("test4");
    end
endmodule
`endif
