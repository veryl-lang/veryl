# Veryl

Veryl is a modern hardware description language.

This project is under the exploration phase of language design.
If you have any idea, please open [Issue](https://github.com/dalance/veryl/issues).

[![Actions Status](https://github.com/dalance/veryl/workflows/Regression/badge.svg)](https://github.com/dalance/veryl/actions)
[![Crates.io](https://img.shields.io/crates/v/veryl.svg)](https://crates.io/crates/veryl)
[![Changelog](https://img.shields.io/badge/changelog-v0.1.1-green.svg)](https://github.com/dalance/veryl/blob/master/CHANGELOG.md)

## Documentation quick links

* [Features](#features)
* [Installation](#installation)
* [Usage](#usage)
* [Reference](#reference)
* [License](#license)
* [Contribution](#contribution)

## Features

* Symplified syntax
    * Based on SystemVerilog / Rust
* Transpiler to SystemVerilog
    * Human readable output
    * Interoperability with SystemVerilog
* Integrated Tools
    * Semantic checker
    * Source code formatter
    * Language server

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
