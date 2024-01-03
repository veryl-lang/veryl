# The Veryl Hardware Description Language

![Veryl](https://github.com/dalance/veryl/raw/master/support/logo/veryl_wide.png)

Veryl is a modern hardware description language which is designed as a "SystemVerilog Alternative".
There are some design concepts.

## Symplified Syntax

Veryl has symplified syntax based on SystemVerilog / Rust.
"Symplified" has two meanings. One is for parser, and another is for human.

SystemVerilog has very complicated syntax (see IEEE Std 1800-2017 Annex A).
This causes difficulty of SystemVerilog tool implementation.
Veryl keeps simple syntax to make tool implementation easier.
So explicit syntax with keyword and symbol is adopted instead of context dependent syntax and shorthand notation.
For example, "off-side rule" like Python, "automatic semicolon insertion" like ECMAScript / Go will not be supported.

SystemVerilog has various syntax. Some syntaxes are inherited from Verilog, and some syntaxes are added from SystemVerilog.
Additionally some syntaxes can be written, but cannot be used actually because major EDA tools don't support them.
So user should learn many syntaxes and whether each syntax can be used or not.
Veryl will not support old Verilog style, unrecommended description, and so on.

## Transpiler to SystemVerilog

HDL alternative languages should be transpiler to the tradisional HDLs like Verilog / VHDL because major EDA tools support them.
Veryl is a transpiler to SystemVerilog.

Transpiler to Verilog has wide EDA tool support including OSS EDA tools.
But even if there are rich data strucuture like `struct` / `interface` in HDL alternatives, transpiled Verilog can't have it.
If HDL alternatives have rich code generateion mechanism, transpiled Verilog will be expanded to the very long code.
For these reason, debugging the transpiled code becomes difficult.

Veryl will has almost all the same semantics as SystemVerilog.
So transpiled code will be human readable SystemVerilog.

Additionally Veryl have interoperability with SystemVerilog.
Veryl can use SystemVerilog's module / interface / struct / enum in the code, and vice versa.

## Integrated Tools

Modern programming languages have development support tools like linter, formatter, and language server by default.
Veryl will have them too from the beginning of development.

The following tools are planed to support.

* Linter
* Formatter
* Language server
* Package manager
