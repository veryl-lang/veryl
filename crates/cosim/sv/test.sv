module test;
    chandle sim;
    logic [127:0] d;

    initial begin
        sim = cosim_open("test.veryl", "Top", 0);

        cosim_set(sim, "a", 128'h1234);
        cosim_step_clock(sim, "clk");
        cosim_get(sim, "b", d);
        $display("%h", d);

        `ifndef VERILATOR
            cosim_set(sim, "a", 128'hxzxzxzxzx);
            cosim_step_clock(sim, "clk");
            cosim_get(sim, "b", d);
            $display("%h", d);
        `endif

        cosim_close(sim);
        $finish;
    end
endmodule
