# Literal

## Integer literal

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

## Floating point literal

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
