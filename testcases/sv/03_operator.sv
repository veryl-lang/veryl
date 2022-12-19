module A ;
    // unary arithmetic
    assign a = +1;
    assign a = -1;

    // unary logical
    assign a = !1;
    assign a = ~1;

    // unary reduce
    assign a = &1;
    assign a = |1;
    assign a = ^1;
    assign a = ~&1;
    assign a = ~|1;
    assign a = ~^1;
    assign a = ^~1;

    // binary arithmetic
    assign a = 1 ** 1;
    assign a = 1 * 1;
    assign a = 1 / 1;
    assign a = 1 % 1;
    assign a = 1 + 1;
    assign a = 1 - 1;

    // binary shift
    assign a = 1 << 1;
    assign a = 1 >> 1;
    assign a = 1 <<< 1;
    assign a = 1 >>> 1;

    // binary compare
    assign a = 1 < 1;
    assign a = 1 <= 1;
    assign a = 1 > 1;
    assign a = 1 >= 1;
    assign a = 1 == 1;
    assign a = 1 != 1;
    assign a = 1 === 1;
    assign a = 1 !== 1;
    assign a = 1 ==? 1;
    assign a = 1 !=? 1;

    // binary bitwise
    assign a = 1 & 1;
    assign a = 1 ^ 1;
    assign a = 1 ~^ 1;
    assign a = 1 ^~ 1;
    assign a = 1 | 1;

    // binary logical
    assign a = 1 && 1;
    assign a = 1 || 1;
endmodule
