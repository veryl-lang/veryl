module veryl_testcase_Module03;
    // unary arithmetic
    logic _a ; always_comb _a = +1;
    logic _aa; always_comb _aa = -1;

    // unary logical
    logic _b ; always_comb _b = !1;
    logic _bb; always_comb _bb = ~1;

    // unary reduce
    logic _c      ; always_comb _c = &1;
    logic _cc     ; always_comb _cc = |1;
    logic _ccc    ; always_comb _ccc = ^1;
    logic _cccc   ; always_comb _cccc = ~&1;
    logic _ccccc  ; always_comb _ccccc = ~|1;
    logic _cccccc ; always_comb _cccccc = ~^1;
    logic _ccccccc; always_comb _ccccccc = ^~1;

    // binary arithmetic
    logic _d     ; always_comb _d = 1 ** 1;
    logic _dd    ; always_comb _dd = 1 * 1;
    logic _ddd   ; always_comb _ddd = 1 / 1;
    logic _dddd  ; always_comb _dddd = 1 % 1;
    logic _ddddd ; always_comb _ddddd = 1 + 1;
    logic _dddddd; always_comb _dddddd = 1 - 1;

    // binary shift
    logic _e   ; always_comb _e = 1 << 1;
    logic _ee  ; always_comb _ee = 1 >> 1;
    logic _eee ; always_comb _eee = 1 <<< 1;
    logic _eeee; always_comb _eeee = 1 >>> 1;

    // binary compare
    logic _f         ; always_comb _f = 1 < 1;
    logic _ff        ; always_comb _ff = 1 <= 1;
    logic _fff       ; always_comb _fff = 1 > 1;
    logic _ffff      ; always_comb _ffff = 1 >= 1;
    logic _fffff     ; always_comb _fffff = 1 == 1;
    logic _ffffff    ; always_comb _ffffff = 1 != 1;
    logic _fffffff   ; always_comb _fffffff = 1 === 1;
    logic _ffffffff  ; always_comb _ffffffff = 1 !== 1;
    logic _fffffffff ; always_comb _fffffffff = 1 ==? 1;
    logic _ffffffffff; always_comb _ffffffffff = 1 !=? 1;

    // binary bitwise
    logic _g    ; always_comb _g = 1 & 1;
    logic _gg   ; always_comb _gg = 1 ^ 1;
    logic _ggg  ; always_comb _ggg = 1 ~^ 1;
    logic _gggg ; always_comb _gggg = 1 ^~ 1;
    logic _ggggg; always_comb _ggggg = 1 | 1;

    // binary logical
    logic _h ; always_comb _h = 1 && 1;
    logic _hh; always_comb _hh = 1 || 1;
endmodule
//# sourceMappingURL=../map/testcases/sv/03_operator.sv.map
