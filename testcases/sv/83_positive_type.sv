module veryl_testcase_Module83;
    // p8 type - unsigned positive 8-bit integer
    byte unsigned _a; always_comb _a = 1;
    byte unsigned _b; always_comb _b = 8'd255;


    // p16 type - unsigned positive 16-bit integer
    shortint unsigned _d; always_comb _d = 1;
    shortint unsigned _e; always_comb _e = 16'd65535;


    // p32 type - unsigned positive 32-bit integer
    int unsigned _g; always_comb _g = 1;
    int unsigned _h; always_comb _h = 32'd4294967295;


    // p64 type - unsigned positive 64-bit integer
    longint unsigned _j; always_comb _j = 1;
    longint unsigned _k; always_comb _k = 64'd18446744073709551615;


    // p* with expressions
    byte unsigned     _m; always_comb _m = 1 + 1;
    shortint unsigned _n; always_comb _n = 10 * 5;
    int unsigned      _o; always_comb _o = 100 + 50;

    // arrays of p* types
    // let _p: p8  [2] = '{1, 255};
    // let _q: p16 [3] = '{1, 100, 65535};
    // let _r: p32 [2] = '{1, 4294967295};

    // in always block
    always_comb begin
        byte unsigned x;
        x = 42;
    end
endmodule
//# sourceMappingURL=../map/83_positive_type.sv.map
