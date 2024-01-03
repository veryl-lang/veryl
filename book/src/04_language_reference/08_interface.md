# Interface

Interface is one of top level components in source code.
Interface has overridable parameters, and interface definitions.

Overridable parameters are the same as them of module.

In interface definitions, `modport` can be declared. 
`modport` can be used as bundled port connection at the port declaration of module.

```veryl,playground
interface InterfaceA #(
    parameter ParamA: u32 = 0,
    parameter ParamB: u32 = 0,
) {
    var a: logic;
    var b: logic;

    modport master {
        a: output,
        b: input ,
    }

    modport slave {
        b: input ,
        a: output,
    }
}
```
