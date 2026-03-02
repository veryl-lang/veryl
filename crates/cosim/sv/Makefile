run_vcs:
	cargo build
	vcs -sverilog -full64 test.sv cosim.sv
	./simv -sv_lib ../../../target/debug/libveryl_cosim

run_verilator:
	cargo build
	verilator -binary test.sv cosim.sv $(realpath ../../../target/debug/libveryl_cosim.so)
	./obj_dir/Vtest

clean:
	rm -rf ./csrc ./obj_dir ./simv.daidir *.log ./simv ./a.out ./ucli.key ./xvlog.* ./xelab.* ./xsim* vc_hdrs.h
