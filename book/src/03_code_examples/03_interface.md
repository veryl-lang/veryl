# Interface

```veryl,playground,editable
// interface definition
interface InterfaceA #(
    parameter ParamA: u32 = 1,
    parameter ParamB: u32 = 1,
) {
    localparam ParamC: u32 = 1;

    var a: logic<ParamA>;
    var b: logic<ParamA>;
    var c: logic<ParamA>;

    // modport definition
    modport master {
        a: input ,
        b: input ,
        c: output,
    }

    modport slave {
        a: input ,
        b: input ,
        c: output,
    }
}

module ModuleA (
    i_clk     : input logic               ,
    i_rst     : input logic               ,
    // port declaration by modport
    intf_a_mst: modport InterfaceA::master,
    intf_a_slv: modport InterfaceA::slave ,
) {
    // interface instantiation
    inst u_intf_a: InterfaceA [10];
}
```
