# Quick Start

Veryl has the almost same semantics as SystemVerilog.
If you are used to SystemVerilog, you will guess Veryl semantics with a small example source code.

In the following example, `[!]` shows the difference with SystemVerilog syntax.

```veryl,playground
import PackageX::*;

// module definition
module ModuleA #(
    parameter  ParamA: u32 = 10, // [!] name is first, and type is followed after `:`
    localparam ParamB: u32 = 10, // [!] trailing comma is allowed
) (
    i_clk: input logic,
    i_rst: input logic,
) { // [!] use `{}` instead of `begin`/`end`

    // localparam declaration
    localparam ParamC: u32 = 10; // [!] `parameter` is not allowed in module

    // variable declaration
    var data: logic [10];

    // always_ff statement
    always_ff (i_clk, i_rst) { // [!] `always_ff` can take a mandatory clock
                               //     and a optional reset
        if_reset {             // [!] `if_reset` means `if (i_rst)`
                               //     This conceals reset porality
            data = 0;          // [!] `=` means non-blocking assignment in `always_ff`
        } else if a {          // [!] `()` of `if` is not required
            data = 0;
        } else {
            data = 0;
        }
    }

    always_ff (i_clk) {
        data = 0;
    }

    // module instantiation
    inst u_module_b: ModuleB #(
    ) (
    );

    // interface instantiation
    inst u_interface_a: InterfaceA;
}

// interface definition
interface InterfaceA #(
    parameter ParamA: u32 = 1,
    parameter ParamB: u32 = 1,
) {
}

// package definition
package PackageA {
}
```
