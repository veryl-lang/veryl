# Contributing to Veryl

Thank you for your interest in contributing to Veryl! We welcome contributions in the following areas:

- Language design
- Tool implementation
- Standard library implementation

Feel free to open [Issues](https://github.com/veryl-lang/veryl/issues) for bug reports, feature requests, or questions. You can also join our [Discord](https://discord.gg/MJZr9NufTT) for discussion.

## Getting Started

You will need a stable Rust toolchain. Then:

```bash
cargo build              # Build the workspace
cargo test               # Run the full test suite
cargo fmt --check        # Check formatting
cargo clippy -- -D warnings  # Lint
```

Please ensure `cargo test` passes locally before submitting a pull request.

## Tests

Alongside the ordinary `#[test]` unit tests in each crate, much of the suite is
file-based: a build script generates one test per `.veryl` file under `testcases/`.
Adding a file-based test is usually just adding a source file and its expected output.

### Positive tests (`testcases/veryl/`)

Code that should compile. The expected SystemVerilog (`testcases/sv/`) and source map
(`testcases/map/`) are committed reference files compared verbatim.

1. Add `testcases/veryl/NN_name.veryl` (follow the existing numbered naming).
2. Regenerate the reference files from the repo root:

   ```bash
   cargo run --bin veryl -- build
   ```

   This writes `testcases/sv/NN_name.sv` and `testcases/map/NN_name.sv.map`.
3. Review the generated output, run `cargo test -p veryl-tests`, then commit the
   `.veryl`, `.sv`, and `.sv.map` files together.

### Error tests (`testcases/error/`)

Code that must fail. The snapshot is the rendered diagnostic, managed with
[`insta`](https://insta.rs/). Only the first reported error is captured, so keep the
case minimal so the target error comes first.

1. Add `testcases/error/my_error.veryl` with minimal code that triggers the error
   (name the module after the file, matching the existing cases).
2. Generate and review the snapshot:

   ```bash
   cargo insta test -p veryl-tests --review   # or: cargo test, then cargo insta review
   ```

3. After accepting, commit both `testcases/error/my_error.veryl` and the generated
   `crates/tests/src/snapshots/my_error.snap`.

## Submitting Changes

1. For non-trivial changes, open an issue first to discuss the approach.
2. Fork the repository and create a feature branch from `master`.
3. Keep commits focused and write descriptive commit messages.
4. Open a pull request. CI must pass before merge.

## Coding Style

- All comments and documentation should be written in English.
- Rust formatting is enforced by `cargo fmt`.
- All clippy warnings are treated as errors (`-D warnings`).

## AI-Assisted Contributions

AI-assisted contributions are permitted, provided the contributor has reviewed, tested, and takes full responsibility for the submitted code. Do not list AI tools as co-authors in commit metadata.

## License

Licensed under either of

- Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
