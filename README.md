# Veryl

Veryl is a modern hardware description language.

This project is under the exploration phase of language design.
If you have any idea, please open [Issue](https://github.com/dalance/veryl/issues).

[![Actions Status](https://github.com/dalance/veryl/workflows/Regression/badge.svg)](https://github.com/dalance/veryl/actions)
[![Crates.io](https://img.shields.io/crates/v/veryl.svg)](https://crates.io/crates/veryl)
[![Changelog](https://img.shields.io/badge/changelog-v0.1.3-green.svg)](https://github.com/dalance/veryl/blob/master/CHANGELOG.md)

## Documentation quick links

* [Concepts](#concepts)
* [Installation](#installation)
* [Usage](#usage)
* [Reference](#reference)
* [License](#license)
* [Contribution](#contribution)

## Concepts

Veryl is designed as a "SystemVerilog Alternative".
There are some design concepts.

### Symplified Syntax

Veryl has symplified syntax based on SystemVerilog / Rust.
"Symplified" has two meanings. One is for parser, and another is for human.

SystemVerilog has very complicated syntax (see IEEE Std 1800-2017 Annex A).
This causes difficulty of SystemVerilog tool implementation.
So Veryl should have simple syntax to parse.
For example, "off-side rule" like Python, "automatic semicolon insertion" like ECMAScript / Go will not be supported.

SystemVerilog has various syntax. Some syntaxes are inherited from Verilog, and some syntaxes are added from SystemVerilog.
Additionally some syntaxes can be written, but cannot be used actually because major EDA tools don't support them.
So user should learn many syntaxes and whether each syntax can be used or not.
Veryl will not support old Verlog style, unrecommended description, and so on.

### Transpiler to SystemVerilog

HDL alternative languages should be transpiler to the tradisional HDLs like Verlog / VHDL because major EDA tools support them.
Veryl is a transpiler to SystemVerilog.

Transpiler to Verilog has wide EDA tool support including OSS EDA tools.
But even if there are rich data strucuture like `struct` / `interface` in HDL alternatives, transpiled Verilog can't have it.
If HDL alternatives have rich code generateion mechanism, transpiled Verilog will be expanded to the very long code.
For these reason, debugging the transpiled code becomes difficult.

Veryl will has almost all the same semantics as SystemVerilog.
So transpiled code will be human readable SystemVerilog.

Additionally Veryl have interoperability with SystemVerilog.
Veryl can use SystemVerilog's module / interface / struct / enum in the code, and vice versa.

### Integrated Tools

Modern programming languages have development support tools like linter, formatter, and language server by default.
Veryl will have them too from the beginning of development.

The following tools are planed to support.

* Semantic checker
* Source code formatter
* Language server
* Package manager

## Installation

### Download binary

Download from [release page](https://github.com/dalance/veryl/releases/latest), and extract to the directory in PATH.

### Cargo

You can install with [cargo](https://crates.io/crates/veryl).

```
cargo install veryl veryl-ls
```

## Usage

* Create a new package

```
veryl new [package name]
```

* Create a new package in an existing directory

```
veryl init [path]
```

* Format the current package

```
veryl fmt
```

* Analyze the current package

```
veryl check
```

* Build target codes corresponding to the current package

```
veryl build
```

## Package Configuration Example

```toml
[package]
name = "name"      # package name
version = "0.1.0"  # package version (semver is recommended)

[build]
clock_type = "posedge"    # default clock type [posedge|negedge]
reset_type = "async_low"  # default reset type [async_low|async_high|sync_low|sync_high]

# output target files in the same location as source
target     = {type = "source"}

# output target files in the specified directory
#target     = {type = "directory", path = "testcases/sv"}

[format]
indent_width = 4  # indent width
```

## Reference

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
