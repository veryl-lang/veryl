# Parameter

Parameter can be declarated as the same as variable.
`parameter` keyword can be used at module header, it can be overridden at instantiation.
`localparam` keyword can be used in module, it can't be overridden.

```veryl,playground
module ModuleA #(
    parameter ParamA: u32 = 1,
) {
    localparam ParamB: u32 = 1;
}
```
