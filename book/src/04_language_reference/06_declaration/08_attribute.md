# Attribute

Attribute can annotate some declarations like variable declaration.

## SV Attribute

SV attribute represents SystemVerilog attribute.
It will be transpiled to SystemVerilog attribute `(*  *)`.

```veryl,playground
module ModuleA {
    #[sv("ram_style=\"block\"")]
    var _a: logic<10>;
    #[sv("mark_debug=\"true\"")]
    var _b: logic<10>;
}
```
