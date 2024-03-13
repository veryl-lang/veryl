module veryl_testcase_Module18;
    logic a;
    logic b;

    always_comb a = {a[10:0], b};
    always_comb a = {{10{a[10:0]}}, {4{b}}};
endmodule
