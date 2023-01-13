# Operator

```veryl
# module ModuleA {
# always_comb {
// unary arithmetic
a = +1;
a = -1;

// unary logical
a = !1;
a = ~1;

// unary reduce
a = &1;
a = |1;
a = ^1;
a = ~&1;
a = ~|1;
a = ~^1;
a = ^~1;

// binary arithmetic
a = 1 ** 1;
a = 1 * 1;
a = 1 / 1;
a = 1 % 1;
a = 1 + 1;
a = 1 - 1;

// binary shift
a = 1 << 1;
a = 1 >> 1;
a = 1 <<< 1;
a = 1 >>> 1;

// binary compare
a = 1 < 1;
a = 1 <= 1;
a = 1 > 1;
a = 1 >= 1;
a = 1 == 1;
a = 1 != 1;
a = 1 === 1;
a = 1 !== 1;
a = 1 ==? 1;
a = 1 !=? 1;

// binary bitwise
a = 1 & 1;
a = 1 ^ 1;
a = 1 ~^ 1;
a = 1 ^~ 1;
a = 1 | 1;

// binary logical
a = 1 && 1;
a = 1 || 1;
# }
# }
```
