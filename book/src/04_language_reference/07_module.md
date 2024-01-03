# Module

Module is one of top level components in source code.
Module has overridable parameters, connection ports, and internal logic.

Overridable parameters can be declared in `#()`.
Each parameter declaration is started by `parameter` keyword.
After the keyword, an identifier, `:`, the type of the parameter, and a default value are placed.

Connection ports can be declared in `()`.
Each port declaration is constructed by an identifier, `:`, port direction, and the type of the port.
The available port directions are:

* `input`: input port
* `output`: output port
* `inout`: bi-directional port
* `modport`: modport of interface

```veryl,playground
module ModuleA #(
    parameter ParamA: u32 = 0,
    parameter ParamB: u32 = 0,
) (
    a: input  logic,
    b: input  logic,
    c: input  logic,
    x: output logic,
) {
    always_comb {
        if c {
            x = a;
        } else {
            x = b;
        }
    }
}
```
