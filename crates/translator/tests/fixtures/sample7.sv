module ternary (
    input  logic       clk,
    input  logic       sel,
    input  logic       en,
    input  logic [7:0] a,
    input  logic [7:0] b,
    output logic [7:0] y,
    output logic [7:0] z,
    output logic [7:0] w,
    output logic [7:0] q
);
    assign y = sel ? a : b;
    assign z = sel ? a : en ? b : 8'h00;
    assign w = a + (sel ? b : 8'h01);
    always_ff @(posedge clk) begin
        q <= en ? a : q;
    end
endmodule
