# component: user component vs SystemVerilog DPI-C

This directory pits Veryl's user-component boundary against the established
SystemVerilog approach — DPI-C calling into a C model — on the same workload.

A 32-bit counter feeds an accumulator model for 1,000,000 cycles:

- `test.veryl` drives the model as a Veryl user component (`$comp::accumulator`),
  read back through the component's `on_clock`.
- `test.sv` drives the identical model through `import "DPI-C"`, the C body in
  `accumulator.c`, called once per posedge.

Both cross a Rust/C function boundary every clock edge. The model is kept
trivial on purpose so the crossing — not the model's own work — dominates the
measurement.

The Veryl side is driven by the `component_compare` example, which embeds the
accumulator as a static (dlopen-free) component. DPI-C is itself an in-process
call, so a static component is the like-for-like comparison; it also keeps the
harness free of a separate component-crate build.

## Running

Requires `verilator` and `hyperfine` on `PATH`:

```
make
```

`make build` compiles both sides (the Veryl runner via `cargo build --release
--example component_compare`, and `test.sv` + `accumulator.c` via Verilator).
`make run` reports wall-clock time for Verilator (DPI) against the Veryl
simulator in two- and four-state modes. Each side prints its final accumulator
value so the two can be cross-checked by eye: both accumulate `0..999999`, so
the result is `sum(0..999999) mod 2^32 = 1783293664` (`0x6a4ae6e0`).
