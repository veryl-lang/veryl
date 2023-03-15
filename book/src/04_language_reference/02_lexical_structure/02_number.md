# Number

## Integer

```veryl
# module A {
# always_comb {
# a =
// integer
0123456789
# +
01_23_45_67_89
# +

// binary
32'b01xzXZ
# +
32'b01_xz_XZ
# +

// octal
32'o01234567xzXZ
# +
32'o01_23_45_67_xz_XZ
# +

// decimal
32'd0123456789
# +
32'd01_23_45_67_89
# +

// hex
128'h0123456789abcdefxzABCDEFXZ
# +
128'h01_23_45_67_89_ab_cd_ef_xz_AB_CD_EF_XZ
# ;
# }
# }
```

## Set all bits

```veryl
# module A {
# always_comb {
# a =
// all 0
'0
# +

// all 1
'1
# +

// all x
'x
# +
'X
# +

// all z
'z
# +
'Z
# ;
# }
# }
```

## Widthless integer

The bit width specification of integer can be omitted.
If it is omitted, the appropriate width will be filled in the translated code.

```veryl,playground
module ModuleA {
    localparam a0: u64 = 'b0101;
    localparam a1: u64 = 'o01234567;
    localparam a2: u64 = 'd0123456789;
    localparam a3: u64 = 'h0123456789fffff;
}
```

## Set sized bits

The bit width specification can be added to "set all bits".

```veryl,playground
module ModuleA {
    localparam a0: u64 = 1'0;
    localparam a1: u64 = 2'1;
    localparam a2: u64 = 3'x;
    localparam a3: u64 = 4'z;
}
```

## Floating point

```veryl
# module A {
# always_comb {
# a =
// floating point
0123456789.0123456789
# +
01_23_45_67_89.01_23_45_67_89
# +

// floating with exponent
0123456789.0123456789e+0123456789
# +
01_23_45_67_89.01_23_45_67_89E-01_23_45_67_89
# ;
# }
# }
```
