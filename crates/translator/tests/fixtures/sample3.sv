typedef enum logic [1:0] {ST_IDLE, ST_RUN, ST_DONE} state_e;

module foo #(
    parameter int WIDTH = 8,
    parameter logic [3:0] INIT = 4'h0
) (
    input  logic              clk,
    input  logic              rst,
    input  logic signed [7:0] a,
    output logic [WIDTH-1:0]  q
);
    logic [WIDTH-1:0] r;

    function logic [WIDTH-1:0] add(input logic [WIDTH-1:0] x, input logic [WIDTH-1:0] y);
        return x + y;
    endfunction

    always_ff @(posedge clk or negedge rst) begin
        if (~rst) begin
            r <= '0;
        end else begin
            for (int i = 0; i < WIDTH; i = i + 1) begin
                r[i] <= a[i];
            end
        end
    end

    assign q = add(r, INIT);
endmodule
