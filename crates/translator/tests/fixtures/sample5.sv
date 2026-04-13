interface bus_if;
    logic       valid;
    logic [7:0] data;
    logic       ready;
    modport master (output valid, output data, input ready);
    modport slave  (input valid, input data, output ready);
endinterface

module producer (
    input  logic    clk,
    bus_if.master   bus
);
    assign bus.valid = 1;
    assign bus.data  = 8'hAB;
endmodule

module wrap (
    input logic clk,
    bus_if      generic_bus
);
    logic a, b;
    assign a = 1, b = 2;
endmodule
