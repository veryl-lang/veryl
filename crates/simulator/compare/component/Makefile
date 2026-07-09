# Compares the user-component boundary (Veryl `$comp`) against SystemVerilog
# DPI-C reaching the identical accumulator model. Both drive a 32-bit counter
# into the model for 1,000,000 cycles. Requires `verilator` and `hyperfine`
# on PATH.

VERYL = ../../../../target/release/examples/component_compare

all: build run clean

build:
	@echo "--------------------------------------------------------------------------------"
	@echo "Build"
	@echo "--------------------------------------------------------------------------------"
	cargo build --release --example component_compare -p veryl-simulator
	verilator -binary test.sv accumulator.c --unroll-count 10000

run:
	@echo "--------------------------------------------------------------------------------"
	@echo "Run Benchmark"
	@echo "--------------------------------------------------------------------------------"
	@hyperfine --min-runs 1 \
		--command-name "Verilator (DPI)" "./obj_dir/Vtest" \
		--command-name "Veryl (2state)"  "$(VERYL) test.veryl --use-jit" \
		--command-name "Veryl (4state)"  "$(VERYL) test.veryl --use-jit --use-4state"

clean:
	rm -rf ./obj_dir *.log
