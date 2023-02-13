![Veryl](support/logo/veryl_wide.png)

[![Actions Status](https://github.com/dalance/veryl/workflows/Regression/badge.svg)](https://github.com/dalance/veryl/actions)
[![Crates.io](https://img.shields.io/crates/v/veryl.svg)](https://crates.io/crates/veryl)
[![Changelog](https://img.shields.io/badge/changelog-v0.3.4-green.svg)](https://github.com/dalance/veryl/blob/master/CHANGELOG.md)

Veryl is a modern hardware description language.

This project is under the exploration phase of language design.
If you have any idea, please open [Issue](https://github.com/dalance/veryl/issues).

* [Document](https://dalance.github.io/veryl/book)
* [PlayGround](https://dalance.github.io/veryl/playground)

## Documentation quick links

* [Concepts](#concepts)
* [Example](#example)
* [Installation](#installation)
* [Usage](#usage)
* [License](#license)
* [Contribution](#contribution)

## Concepts

Veryl is designed as a "SystemVerilog Alternative".
There are some design concepts.

* Symplified syntax
    * Based on SystemVerilog / Rust
    * Removed traditional Verilog syntax
* Transpiler to SystemVerilog
    * Human readable SystemVerilog code generation
    * Interoperability with SystemVerilog
* Integrated tools
    * Formatter / Linter
    * VSCode, vim/neovim integration
    * Package management based on git

## Example

```
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

## Installation

### Requirement

Veryl uses `git` command internally.

### Instruction

#### Download binary

Download from [release page](https://github.com/dalance/veryl/releases/latest), and extract to the directory in PATH.

#### Cargo

You can install with [cargo](https://crates.io/crates/veryl).

```
cargo install veryl veryl-ls
```

### Editor integration

* [vim/neovim plugin](https://github.com/dalance/veryl.vim)
* [VSCode plugin](https://marketplace.visualstudio.com/items?itemName=dalance.vscode-veryl)

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
```

### Package Configuration

```toml
[project]
name = "name"      # project name
version = "0.1.0"  # project version (semver is recommended)

[build]
clock_type    = "posedge"    # default clock type [posedge|negedge]
reset_type    = "async_low"  # default reset type [async_low|async_high|sync_low|sync_high]
filelist_type = "absolute"   # filelist type [absolute|relative|flgen]

# output target files in the same location as source
target     = {type = "source"}

# output target files in the specified directory
#target     = {type = "directory", path = "testcases/sv"}

[format]
indent_width = 4  # indent width
```

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
