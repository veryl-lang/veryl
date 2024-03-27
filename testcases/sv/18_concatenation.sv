module veryl_testcase_Module18;
    logic a;
    logic b;
    logic c;
    always_comb c = 1;

    always_comb a = {a[10:0], c};
    always_comb b = {{10{a[10:0]}}, {4{c}}};
endmodule
