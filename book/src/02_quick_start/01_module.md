# Module

* simple module definition

```veryl,playground
// module definition
module ModuleA #(
    parameter  ParamA: u32 = 10,
    localparam ParamB: u32 = 10, // trailing comma is allowed
) (
    i_clk : input  logic,
    i_rst : input  logic,
    i_sel : input  logic,
    i_data: input  logic<ParamA> [2], // `[]` means unpacked array in SystemVerilog
    o_data: output logic<ParamA>    , // `<>` means packed array in SystemVerilog
) {
    // localparam declaration
    //   `parameter` is not allowed in module
    localparam ParamC: u32 = 10;

    // variable declaration
    var r_data0: logic<ParamA>;
    var r_data1: logic<ParamA>;

    // always_ff statement with reset
    //   `always_ff` can take a mandatory clock and a optional reset
    //   `if_reset` means `if (i_rst)`. This conceals reset porality
    //   `()` of `if` is not required
    //   `=` in `always_ff` is non-blocking assignment
    always_ff (i_clk, i_rst) {
        if_reset {
            r_data0 = 0;
        } else if i_sel {
            r_data0 = i_data[0];
        } else {
            r_data0 = i_data[1];
        }
    }

    // always_ff statement without reset
    always_ff (i_clk) {
        r_data1 = r_data0;
    }

    assign o_data = r_data1;
}
```

* module instantiation

```veryl,playground
module ModuleA #(
    parameter  ParamA: u32 = 10,
) (
    i_clk : input  logic,
    i_rst : input  logic,
    i_data: input  logic<ParamA>,
    o_data: output logic<ParamA>,
) {
    var r_data1: logic<ParamA>;
    var r_data2: logic<ParamA>;

    assign r_data1 = i_data + 1;
    assign o_data  = r_data2 + 2;

    // instance declaration
    //   `inst` keyword starts instance declaration
    //   port connnection can be specified by `()`
    //   each port connection is `[port_name]:[variable]`
    //   `[port_name]` means `[port_name]:[port_name]`
    inst u_module_b: ModuleB (
        i_clk,
        i_rst,
        i_data: r_data1,
        o_data: r_data2,
    );

    // instance declaration with parameter override
    //   notation of parameter connection is the same as port
    inst u_module_c: ModuleC #(
        ParamA,
        ParamB: 10,
    ) (
        i_clk,
        i_rst,
        i_data: r_data1,
        o_data: r_data2,
    );
}
```
