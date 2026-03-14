module veryl_testcase_Module84;
    logic [((3) > (1) ? (3) : (1))-1:0] foo;
    logic [((3) < (1) ? (3) : (1))-1:0] bar;

    always_comb foo = '0;
    always_comb bar = '0;
endmodule
//# sourceMappingURL=../map/84_min_max.sv.map
