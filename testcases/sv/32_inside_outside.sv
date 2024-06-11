module veryl_testcase_Module32;
    logic a;
    logic b;

    always_comb a = ((1 + 2 / 3) inside {0, [0:(10)-1], [1:10]});
    always_comb b = !((1 * 2 - 1) inside {0, [0:(10)-1], [1:10]});
endmodule
//# sourceMappingURL=../map/testcases/sv/32_inside_outside.sv.map
