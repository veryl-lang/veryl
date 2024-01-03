# Register

If a variable is assigned in `always_ff` declaration, it becomes register variable.
Register variable will be mapped to flip-flop in synthesis phase.

`always_ff` has mandatory clock variable, optional reset variable, and `{}` block.
Clock and reset are placed in `()`.

`if_reset` is a special keyword which can be used in `always_ff`.
It means reset condition of the register variable.
If `if_reset` is used, `always_ff` must have reset variable.
`if_reset` can be conceal reset porality and synchronisity.
The actual porality and synchronisity can be configured through `[build]` section of `Veryl.toml`.

```veryl,playground
module ModuleA (
    i_clk: input logic,
    i_rst: input logic,
) {
    var a: logic<10>;
    var b: logic<10>;

    always_ff (i_clk) {
        a = 1;
    }

    always_ff (i_clk, i_rst) {
        if_reset {
            b = 0;
        } else {
            b = 1;
        }
    }
}
```
