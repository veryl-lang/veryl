# Quick Start

Veryl has the almost same semantics as SystemVerilog.
If you are used to SystemVerilog, you will guess Veryl semantics with a small example source code.

This is a small example.
In the following example,  comments show the difference with SystemVerilog syntax.

```veryl,playground
module ModuleA (
    // name is first, and type is followed after `:`
    // bit width is represented by `<>`
    i_data: input  logic<10>,
    o_data: output logic<10>,

    // use `{}` instead of `begin`/`end`
) {
    assign o_data = i_data;
}
```

Some examples in the book have play button "▶" which will be appeared when mouse cursor is hovered at the code.
If you click the button, the transpiled SystemVerilog code will appear.

A source code of Veryl has some `module`, `interface` and `package` like SystemVerilog.
In this chapter, we'll show the some example source codes of them.
