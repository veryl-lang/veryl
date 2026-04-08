typedef logic [7:0] byte_t;

package my_pkg;
    typedef enum logic [1:0] { S_IDLE, S_RUN, S_DONE } state_e;
    parameter int WIDTH = 16;
endpackage

interface bus_if;
    logic       valid;
    logic [7:0] data;
    logic       ready;

    modport master (output valid, output data, input ready);
    modport slave  (input valid, input data, output ready);
endinterface

module dut #(parameter int N = 4) (
    input  logic              clk,
    input  logic [N-1:0][7:0] mem,
    output logic [N-1:0]      flags
);
    logic [3:0][7:0] regfile;

    always_comb begin
        for (int i = 0; i < N; i = i + 1) begin
            flags[i] = (mem[i] > 0);
        end
    end
endmodule
