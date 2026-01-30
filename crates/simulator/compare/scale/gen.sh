#!/bin/bash
# Generate test.veryl and test.sv with N unrolled RiscvCore instances
set -eu

N=${1:?Usage: gen.sh <N_CORES>}
DIR="$(cd "$(dirname "$0")" && pwd)"

# --- Generate test.veryl ---
{
    # Copy RiscvCore module from cpu/test.veryl (lines 1-513)
    sed -n '1,513p' "$DIR/../cpu/test.veryl"

    echo ""
    echo "module Top ("
    echo "    clk: input  clock     ,"
    echo "    rst: input  reset     ,"
    echo "    out: output logic<32> ,"
    echo ") {"

    # Declare per-core output variables
    for i in $(seq 0 $((N-1))); do
        echo "    var core_out_${i}: logic<32>;"
    done
    echo ""

    # Instantiate cores
    for i in $(seq 0 $((N-1))); do
        echo "    inst u_core_${i}: RiscvCore ("
        echo "        clk,"
        echo "        rst,"
        echo "        out: core_out_${i},"
        echo "    );"
    done
    echo ""

    # XOR reduction (unrolled)
    echo "    always_comb {"
    printf "        out = core_out_0"
    for i in $(seq 1 $((N-1))); do
        printf " ^ core_out_${i}"
    done
    echo ";"
    echo "    }"
    echo "}"
} > "$DIR/test.veryl"

# --- Generate test.sv ---
{
    # Copy test (testbench) module from cpu/test.sv (lines 1-34)
    sed -n '1,34p' "$DIR/../cpu/test.sv"

    echo ""

    # Copy riscv_core module from cpu/test.sv (lines 36-512)
    sed -n '36,512p' "$DIR/../cpu/test.sv"

    echo ""
    echo "module top ("
    echo "    input  var        i_clk,"
    echo "    input  var        i_rst,"
    echo "    output var [31:0] o_out"
    echo ");"

    # Declare per-core output wires
    for i in $(seq 0 $((N-1))); do
        echo "    logic [31:0] core_out_${i};"
    done
    echo ""

    # Instantiate cores
    for i in $(seq 0 $((N-1))); do
        echo "    riscv_core u_core_${i} ("
        echo "        .i_clk (i_clk),"
        echo "        .i_rst (i_rst),"
        echo "        .o_out (core_out_${i})"
        echo "    );"
    done
    echo ""

    # XOR reduction (unrolled)
    printf "    assign o_out = core_out_0"
    for i in $(seq 1 $((N-1))); do
        printf " ^ core_out_${i}"
    done
    echo ";"
    echo "endmodule"
} > "$DIR/test.sv"

echo "Generated test.veryl and test.sv with N=$N cores"
