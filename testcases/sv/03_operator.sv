module Module03 ;
    logic  a         ;
    logic  aa        ;
    logic  aaa       ;
    logic  aaaa      ;
    logic  aaaaa     ;
    logic  aaaaaa    ;
    logic  aaaaaaa   ;
    logic  aaaaaaaa  ;
    logic  aaaaaaaaa ;
    logic  aaaaaaaaaa;

    // unary arithmetic
    assign a  = +1;
    assign aa = -1;

    // unary logical
    assign a  = !1;
    assign aa = ~1;

    // unary reduce
    assign a       = &1;
    assign aa      = |1;
    assign aaa     = ^1;
    assign aaaa    = ~&1;
    assign aaaaa   = ~|1;
    assign aaaaaa  = ~^1;
    assign aaaaaaa = ^~1;

    // binary arithmetic
    assign a      = 1 ** 1;
    assign aa     = 1 * 1;
    assign aaa    = 1 / 1;
    assign aaaa   = 1 % 1;
    assign aaaaa  = 1 + 1;
    assign aaaaaa = 1 - 1;

    // binary shift
    assign a    = 1 << 1;
    assign aa   = 1 >> 1;
    assign aaa  = 1 <<< 1;
    assign aaaa = 1 >>> 1;

    // binary compare
    assign a          = 1 < 1;
    assign aa         = 1 <= 1;
    assign aaa        = 1 > 1;
    assign aaaa       = 1 >= 1;
    assign aaaaa      = 1 == 1;
    assign aaaaaa     = 1 != 1;
    assign aaaaaaa    = 1 === 1;
    assign aaaaaaaa   = 1 !== 1;
    assign aaaaaaaaa  = 1 ==? 1;
    assign aaaaaaaaaa = 1 !=? 1;

    // binary bitwise
    assign a     = 1 & 1;
    assign aa    = 1 ^ 1;
    assign aaa   = 1 ~^ 1;
    assign aaaa  = 1 ^~ 1;
    assign aaaaa = 1 | 1;

    // binary logical
    assign a  = 1 && 1;
    assign aa = 1 || 1;
endmodule
