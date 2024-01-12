# Code Examples

Veryl has the almost same semantics as SystemVerilog.
If you are used to SystemVerilog, you will guess Veryl semantics with a small example source code.

This is a small example.
In the following example, comments show the difference with SystemVerilog syntax.

```veryl,playground,editable
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

Additionally, the codeblocks in this chapter can be edit. Let's try to edit and play each code.

A source code of Veryl has some `module`, `interface` and `package` like SystemVerilog.
In this chapter, we'll show the some example source codes of them.
