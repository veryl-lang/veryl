module casts (
    input  logic [7:0] a,
    input  logic       clk,
    output logic [7:0] q
);
    logic [15:0] tmp;
    logic [7:0]  cast_out;

    always_ff @(posedge clk) begin
        tmp <= 16'(a);
        $display("a=%h", a);
    end

    assign cast_out = 8'(tmp + 1);
    assign q = cast_out;
endmodule
