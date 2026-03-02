all: setup run clean

CLEAN_VERILATOR = rm -rf ./obj_dir
CLEAN_VCS       = rm -rf ./csrc ./simv.daidir simv
CLEAN_VIVADO    = rm -rf ./xsim.dir
CLEAN_ICARUS    = rm -rf a.out
CLEAN_VERYL     = 

SETUP_VERILATOR = verilator -binary test.sv --unroll-count 10000
SETUP_VCS       = vcs -sverilog -full64 test.sv
SETUP_VIVADO    = xvlog --sv test.sv; xelab test
SETUP_ICARUS    = iverilog -g2005-sv test.sv
SETUP_VERYL     = ../../../target/release/run test.veryl --cycle 0 --use-jit
ALL_SETUP       = $(SETUP_VERILATOR); $(SETUP_VCS); $(SETUP_VIVADO); $(SETUP_ICARUS); $(SETUP_VERYL)

RUN_VERILATOR    = ./obj_dir/Vtest
RUN_VCS          = ./simv
RUN_VIVADO       = xsim "work.test" --R
RUN_ICARUS       = ./a.out
RUN_VERYL_2STATE = ../../../target/release/run test.veryl --cycle 1000000 --use-jit
RUN_VERYL_4STATE = ../../../target/release/run test.veryl --cycle 1000000 --use-jit --use-4state

HYPERFINE_SETUP += --prepare "$(CLEAN_VERILATOR)" --command-name "Verilator" "$(SETUP_VERILATOR)"
HYPERFINE_SETUP += --prepare "$(CLEAN_VCS)"       --command-name "VCS"       "$(SETUP_VCS)"
HYPERFINE_SETUP += --prepare "$(CLEAN_VIVADO)"    --command-name "Vivado"    "$(SETUP_VIVADO)"
HYPERFINE_SETUP += --prepare "$(CLEAN_ICARUS)"    --command-name "Icarus"    "$(SETUP_ICARUS)"
HYPERFINE_SETUP += --prepare "$(CLEAN_VERYL)"     --command-name "Veryl"     "$(SETUP_VERYL)"

HYPERFINE_RUN += --setup "$(ALL_SETUP)"
HYPERFINE_RUN += --command-name "Verilator"      "$(RUN_VERILATOR)"
HYPERFINE_RUN += --command-name "VCS"            "$(RUN_VCS)"
HYPERFINE_RUN += --command-name "Vivado"         "$(RUN_VIVADO)"
#HYPERFINE_RUN += --command-name "Icarus"         "$(RUN_ICARUS)"
HYPERFINE_RUN += --command-name "Veryl (2state)" "$(RUN_VERYL_2STATE)"
HYPERFINE_RUN += --command-name "Veryl (4state)" "$(RUN_VERYL_4STATE)"

setup:
	@echo "--------------------------------------------------------------------------------"
	@echo "Setup Benchmark"
	@echo "--------------------------------------------------------------------------------"
	@hyperfine --min-runs 1 $(HYPERFINE_SETUP)

run:
	@echo "--------------------------------------------------------------------------------"
	@echo "Run Benchmark"
	@echo "--------------------------------------------------------------------------------"
	@hyperfine --min-runs 1 $(HYPERFINE_RUN)

clean:
	rm -rf ./csrc ./obj_dir ./simv.daidir *.log ./simv ./a.out ./ucli.key ./xvlog.* ./xelab.* ./xsim*
