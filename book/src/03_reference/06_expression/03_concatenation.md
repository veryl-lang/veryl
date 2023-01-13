# Concatenation

```veryl
# module ModuleA {
# always_comb {
a = {b[10:0], c[4:3]};
a = {b[10:0] repeat 10, c repeat 4};
# }
# }
```
