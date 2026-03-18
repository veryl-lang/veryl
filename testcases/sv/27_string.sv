module veryl_testcase_Module27;
    localparam string a = "aaa";

    string _b; always_comb _b = "bbb";

    // string comparison
    logic _c; always_comb _c = "abc" == "abc";
    logic _d; always_comb _d = "abc" != "def";
    logic _e; always_comb _e = "abc" < "abd";
    logic _f; always_comb _f = "b" >= "a";

    // string vs integer comparison
    logic _g; always_comb _g = "A" == 8'h41;
    logic _h; always_comb _h = "AB" != 16'h4142;
endmodule
//# sourceMappingURL=../map/27_string.sv.map
