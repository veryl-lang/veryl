#include <stdint.h>

// Accumulates its input across calls; the SystemVerilog testbench invokes
// this once per posedge through DPI-C, the exact counterpart of the Veryl
// `$comp::accumulator` in test.veryl. Kept deliberately cheap so the DPI
// boundary crossing dominates the measurement.
//
// Verilator compiles this file with the C++ toolchain, so the export needs
// C linkage to match the DPI import's declaration.
#ifdef __cplusplus
extern "C" {
#endif

static uint32_t acc = 0;

int accumulator_step(int d) {
    acc += (uint32_t)d;
    return (int)acc;
}

#ifdef __cplusplus
}
#endif
