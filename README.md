[![Veryl](support/logo/veryl_wide.png)](https://veryl-lang.org/)

[![Actions Status](https://github.com/veryl-lang/veryl/workflows/Regression/badge.svg)](https://github.com/veryl-lang/veryl/actions)
[![Crates.io](https://img.shields.io/crates/v/veryl.svg)](https://crates.io/crates/veryl)
[![Changelog](https://img.shields.io/badge/changelog-v0.11.0-green.svg)](https://github.com/veryl-lang/veryl/blob/master/CHANGELOG.md)

Veryl is a modern hardware description language.

This project is under the exploration phase of language design.
If you have any idea, please open [Issue](https://github.com/veryl-lang/veryl/issues).

* [Document](https://doc.veryl-lang.org/book/)
    * [日本語](https://doc.veryl-lang.org/book/ja/)
* [PlayGround](https://doc.veryl-lang.org/playground/)

## Documentation quick links

* [Concepts](#concepts)
* [Example](#example)
* [Installation](#installation)
* [Usage](#usage)
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
    i_clk : input clock       ,
    i_rst : input reset       ,
    i_data: input logic<WIDTH>,
    o_data: input logic<WIDTH>,
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

## Installation

See [Document](https://doc.veryl-lang.org/book/03_getting_started/01_installation.html).

## Usage

```
// Create a new project
veryl new [project name]

// Create a new project in an existing directory
veryl init [path]

// Format the current project
veryl fmt

// Analyze the current project
veryl check

// Build target codes corresponding to the current project
veryl build

// Build the document corresponding to the current project
veryl doc
```

For detailed information, see [Document](https://doc.veryl-lang.org/book/).

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
