[![Veryl](support/logo/veryl_wide.png)](https://veryl-lang.org/)

[![Actions Status](https://github.com/veryl-lang/veryl/workflows/Regression/badge.svg)](https://github.com/veryl-lang/veryl/actions)
[![Crates.io](https://img.shields.io/crates/v/veryl.svg)](https://crates.io/crates/veryl)
[![CodSpeed Badge](https://img.shields.io/endpoint?url=https://codspeed.io/badge.json)](https://codspeed.io/veryl-lang/veryl)

Veryl is a modern hardware description language.

This project is under the exploration phase of language design.
We call for the following suggestion or contribution:

* Language design
* Tool implementation
* Standard library implementation

If you have any idea, please open [Issue](https://github.com/veryl-lang/veryl/issues) or [Pull request](https://github.com/veryl-lang/veryl/pulls).

## External resources

* [Language Reference](https://doc.veryl-lang.org/book/)
    * [日本語](https://doc.veryl-lang.org/book/ja/)
* [PlayGround](https://doc.veryl-lang.org/playground/)

## Documentation quick links

* [Overview](#overview)
* [Example](#example)
* [FAQ](#faq)
* [Installation & Usage](#installation--usage)
* [Publications](#plublications)
* [Related Projects](#related-projects)
* [License](#license)
* [Contribution](#contribution)

## Overview

Veryl is a hardware description language based on SystemVerilog, providing the following advantages:

### Optimized Syntax
Veryl adopts syntax optimized for logic design while being based on a familiar basic syntax for SystemVerilog experts.
This optimization includes guarantees for synthesizability, ensuring consistency between simulation results, and providing numerous syntax simplifications for common idioms.
This approach enables ease of learning, improves the reliability and efficiency of the design process, and facilitates ease of code writing.

### Interoperability
Designed with interoperability with SystemVerilog in mind, Veryl allows smooth integration and partial replacement with existing SystemVerilog components and projects.
Furthermore, SystemVerilog source code transpiled from Veryl retains high readability, enabling seamless integration and debugging.

### Productivity
Veryl comes with a rich set of development support tools, including package managers, build tools, real-time checkers compatible with major editors such as VSCode, Vim, Emacs, automatic completion, and automatic formatting.
These tools accelerate the development process and significantly enhance productivity.

With these features, Veryl provides powerful support for designers to efficiently and productively conduct high-quality hardware design.

## Example

<table>
<tr>
<th>Veryl</th>
<th>SystemVerilog</th>

</tr>
<tr>
<td>

```systemverilog
/// documentation comment by markdown format
/// * list item1
/// * list item2
pub module Delay #( // visibility control by `pub` keyword
    param WIDTH: u32 = 1, // trailing comma is allowed
) (
    i_clk : input  clock       ,
    i_rst : input  reset       ,
    i_data: input  logic<WIDTH>,
    o_data: output logic<WIDTH>,
) {
    // unused variable which is not started with `_` are warned
    var _unused_variable: logic;

    // clock and reset signals can be omitted
    // because Veryl can infer these signals
    always_ff {
        // abstraction syntax of reset polarity and synchronicity
        if_reset {
            o_data = '0;
        } else {
            o_data = i_data;
        }
    }
}
```

</td>
<td>

```systemverilog
// comment
//
//
module Delay #(
    parameter int WIDTH = 1
) (
    input              i_clk ,
    input              i_rst ,
    input  [WIDTH-1:0] i_data,
    output [WIDTH-1:0] o_data
);
    logic unused_variable;

    always_ff @ (posedge i_clk or negedge i_rst) begin
        if (!i_rst) begin
            o_data <= '0;
        end else begin
            o_data <= i_data;
        end
    end
endmodule
```

</td>
</tr>
</table>

## FAQ

### Why not SystemVerilog?

SystemVerilog is very complicated language, and it causes difficulty of implementing EDA tools for it.
As a consequence, major EDA tools only support SystemVerilog subset which is different each other,
and users must explore usable languege features which are covered by adopted tools.
Additionally, the difficulty prevents productivity improvement by developing support tools.
This is a reason that a new language having simplified and sophisticated syntax, not SystemVerilog, is required.

### Why not existing Alt-HDLs (e.g. Chisel)?

Many existing alt-HDLs are inner DSL of a programming language.
This approach has some advantages like rapid development and resusable tooling ecosystem,
but the syntax can't be fit for hardware description completely.
Additionally, enormous Verilog code is generated from short and sophisticated code in these languages.
This prevents general ASIC workflows like timing improvement, pre/post-mask ECO because these workflows require FF-level modification in Verilog.
Interopration between these language and SystemVerilog is challenging because these can't connect to SystemVerilog's type like `interface` and `struct` directly.
By these reason, the existing Alt-HDLs can't be used as alternative of SystemVerilog, especially if there are many existing SystemVerilog codebase.
Veryl resolves these problems by HDL-specialized syntax and human-readable SystemVerilog code generation.

### Why some language features (e.g. auto pipelining) are not adopted?

Veryl focuses equivalency with SystemVerilog at the point of view of the language semantics.
This eases to predict the changes of generated SystemVerilog code from modification of Veryl code,
and Veryl can be applied to ASIC workflows like timing improvement and pre/post-mask ECO.
Therefore, some features generating FFs are not adopted because these prevent the predictability.

### Why some syntax features (e.g. off-side rule, semicolon less) are not adopted?

Veryl focuses syntax simplicity because it reduces tool implementation effort.
Therefore syntax features which introduce large complexity in exchange for slight abbreviation are not adopted.

## Installation & Usage

See [Getting Started](https://doc.veryl-lang.org/book/03_getting_started.html).

## Publications

* Naoya Hatta, Taichi Ishitani, Ryota Shioya.
  Veryl: A New Hardware Description Language as an Alternative to SystemVerilog.
  August 2024. In: The Design & Verification Conference (DVCon) Japan 2024.
  [[Paper]](https://veryl-lang.org/docs/veryl_dvcon-jpn-2024.pdf)
  [[Slides]](https://veryl-lang.org/docs/veryl_dvcon-jpn-2024-slide.pdf)
  [[arXiv]](http://arxiv.org/abs/2411.12983)

## Related Projects

* [RgGen](https://github.com/rggen/rggen)
    * RgGen is an open source CSR automation tool.
      It can generate CSR modules written in Veryl from readable register map specifications.

## License

Licensed under either of

 * Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
