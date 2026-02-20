module veryl_testcase_Module51;
    logic _a [2]; always_comb _a = '{1, 1};
    logic _b [2]; always_comb _b = '{2{1}};
    logic _c [2]; always_comb _c = '{default: 1};
    logic _d [4]; always_comb _d = '{
        1, 1,
        1, 1
    };
endmodule
//# sourceMappingURL=../map/51_array_literal.sv.map
