module veryl_testcase_Module32;
    logic a;
    logic b;

    assign a = (1 + 2 / 3 inside {0, [0:(10)-1], [1:10]});
    assign b = !(1 * 2 - 1 inside {0, [0:(10)-1], [1:10]});
endmodule
