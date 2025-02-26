module veryl_testcase_Module64;
    int unsigned a; always_comb a = 1;

    int unsigned     _x0; always_comb _x0 = unsigned'(int'(a));
    longint unsigned _x1; always_comb _x1 = unsigned'(longint'(a));
    int signed       _x2; always_comb _x2 = signed'(int'(a));
    longint signed   _x3; always_comb _x3 = signed'(longint'(a));
    shortreal        _x4; always_comb _x4 = shortreal'(a);
    real             _x5; always_comb _x5 = real'(a);
endmodule
//# sourceMappingURL=../map/testcases/sv/64_cast_to_builtin.sv.map
