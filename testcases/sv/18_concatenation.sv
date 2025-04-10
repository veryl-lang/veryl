module veryl_testcase_Module18;
    logic [20-1:0] a;
    logic          b;
    logic          c; always_comb c = 1;

    always_comb a = {a[10:0], c};
    always_comb b = {{10{a[10:0]}}, {4{c}}};

    // multi-line concatenation
    logic _d; always_comb _d = {
        {8{a}}, {8{b}}, {8{c}},
        {8{a}}, {8{b}}, {8{c}},
        {8{a}}, {8{b}}, {8{c}},
        {8{a}}, {8{b}}, {8{c}},
        {8{a}}, {8{b}}, {8{c}},
        {8{a}}, {8{b}}, {8{c}},
        {8{a}}, {8{b}}, {8{c}}
    };

    logic [20-1:0] d;
    logic          e;
    always_comb {d, e} = a;

    veryl_testcase_Module18A u (
        .a ({
            1'b1,
            2'b1,
            1'b1
        }),
        .b (0)
    );
endmodule

module veryl_testcase_Module18A (
    input var logic a,
    input var logic b
);
endmodule
//# sourceMappingURL=../map/18_concatenation.sv.map
