N ?= 4

all: gen setup run clean

CLEAN_VERILATOR = rm -rf ./obj_dir
CLEAN_VCS       = rm -rf ./csrc ./simv.daidir simv
CLEAN_VIVADO    = rm -rf ./xsim.dir
CLEAN_ICARUS    = rm -rf a.out
CLEAN_VERYL     =

SETUP_VERILATOR = verilator -binary test.sv --unroll-count 10000
SETUP_VCS       = vcs -sverilog -full64 test.sv
SETUP_VIVADO    = xvlog --sv test.sv; xelab test
SETUP_ICARUS    = iverilog -g2005-sv test.sv
SETUP_VERYL     = ../../../../target/release/run test.veryl --cycle 0 --use-jit
ALL_SETUP       = $(SETUP_VERILATOR); $(SETUP_VCS); $(SETUP_VIVADO); $(SETUP_ICARUS); $(SETUP_VERYL)

RUN_VERILATOR    = ./obj_dir/Vtest
RUN_VCS          = ./simv
RUN_VIVADO       = xsim "work.test" --R
RUN_ICARUS       = ./a.out
RUN_VERYL_2STATE = ../../../../target/release/run test.veryl --cycle 1000000 --use-jit
RUN_VERYL_4STATE = ../../../../target/release/run test.veryl --cycle 1000000 --use-jit --use-4state

HYPERFINE_SETUP += --prepare "$(CLEAN_VERILATOR)" --command-name "Verilator (N=$(N))" "$(SETUP_VERILATOR)"
#HYPERFINE_SETUP += --prepare "$(CLEAN_VCS)"       --command-name "VCS (N=$(N))"       "$(SETUP_VCS)"
#HYPERFINE_SETUP += --prepare "$(CLEAN_VIVADO)"    --command-name "Vivado (N=$(N))"    "$(SETUP_VIVADO)"
#HYPERFINE_SETUP += --prepare "$(CLEAN_ICARUS)"    --command-name "Icarus (N=$(N))"    "$(SETUP_ICARUS)"
HYPERFINE_SETUP += --prepare "$(CLEAN_VERYL)"     --command-name "Veryl (N=$(N))"     "$(SETUP_VERYL)"

HYPERFINE_RUN += --setup "$(ALL_SETUP)"
HYPERFINE_RUN += --command-name "Verilator (N=$(N))"      "$(RUN_VERILATOR)"
#HYPERFINE_RUN += --command-name "VCS (N=$(N))"            "$(RUN_VCS)"
#HYPERFINE_RUN += --command-name "Vivado (N=$(N))"         "$(RUN_VIVADO)"
#HYPERFINE_RUN += --command-name "Icarus (N=$(N))"         "$(RUN_ICARUS)"
HYPERFINE_RUN += --command-name "Veryl 2state (N=$(N))"   "$(RUN_VERYL_2STATE)"
HYPERFINE_RUN += --command-name "Veryl 4state (N=$(N))"   "$(RUN_VERYL_4STATE)"

gen:
	./gen.sh $(N)

setup: gen
	@echo "--------------------------------------------------------------------------------"
	@echo "Setup Benchmark (N=$(N))"
	@echo "--------------------------------------------------------------------------------"
	@hyperfine --min-runs 1 $(HYPERFINE_SETUP)

run: gen
	@echo "--------------------------------------------------------------------------------"
	@echo "Run Benchmark (N=$(N))"
	@echo "--------------------------------------------------------------------------------"
	@hyperfine --min-runs 1 $(HYPERFINE_RUN)

sweep:
	@for n in 1 2 4 8 16 32 64 128 256 512 1024; do \
		echo "================================================================================"; \
		echo "N=$$n"; \
		echo "================================================================================"; \
		$(MAKE) setup N=$$n; \
		$(MAKE) run N=$$n; \
	done

clean:
	rm -rf ./csrc ./obj_dir ./simv.daidir *.log ./simv ./a.out ./ucli.key ./xvlog.* ./xelab.* ./xsim* test.veryl test.sv
