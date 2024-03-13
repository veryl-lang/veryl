module veryl_testcase_Module03;
    logic a         ;
    logic aa        ;
    logic aaa       ;
    logic aaaa      ;
    logic aaaaa     ;
    logic aaaaaa    ;
    logic aaaaaaa   ;
    logic aaaaaaaa  ;
    logic aaaaaaaaa ;
    logic aaaaaaaaaa;

    // unary arithmetic
    always_comb a  = +1;
    always_comb aa = -1;

    // unary logical
    always_comb a  = !1;
    always_comb aa = ~1;

    // unary reduce
    always_comb a       = &1;
    always_comb aa      = |1;
    always_comb aaa     = ^1;
    always_comb aaaa    = ~&1;
    always_comb aaaaa   = ~|1;
    always_comb aaaaaa  = ~^1;
    always_comb aaaaaaa = ^~1;

    // binary arithmetic
    always_comb a      = 1 ** 1;
    always_comb aa     = 1 * 1;
    always_comb aaa    = 1 / 1;
    always_comb aaaa   = 1 % 1;
    always_comb aaaaa  = 1 + 1;
    always_comb aaaaaa = 1 - 1;

    // binary shift
    always_comb a    = 1 << 1;
    always_comb aa   = 1 >> 1;
    always_comb aaa  = 1 <<< 1;
    always_comb aaaa = 1 >>> 1;

    // binary compare
    always_comb a          = 1 < 1;
    always_comb aa         = 1 <= 1;
    always_comb aaa        = 1 > 1;
    always_comb aaaa       = 1 >= 1;
    always_comb aaaaa      = 1 == 1;
    always_comb aaaaaa     = 1 != 1;
    always_comb aaaaaaa    = 1 === 1;
    always_comb aaaaaaaa   = 1 !== 1;
    always_comb aaaaaaaaa  = 1 ==? 1;
    always_comb aaaaaaaaaa = 1 !=? 1;

    // binary bitwise
    always_comb a     = 1 & 1;
    always_comb aa    = 1 ^ 1;
    always_comb aaa   = 1 ~^ 1;
    always_comb aaaa  = 1 ^~ 1;
    always_comb aaaaa = 1 | 1;

    // binary logical
    always_comb a  = 1 && 1;
    always_comb aa = 1 || 1;
endmodule
