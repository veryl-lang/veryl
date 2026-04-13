module bar (
    input  logic       clk,
    input  logic       rst,
    input  logic [1:0] sel,
    input  logic [7:0] a,
    input  logic [7:0] b,
    output logic [7:0] y
);
    logic [7:0] r;
    always_ff @(posedge clk or negedge rst) begin
        if (!rst) begin
            r <= 8'h00;
        end else begin
            case (sel)
                2'd0: r <= a;
                2'd1: r <= b;
                2'd2, 2'd3: r <= a + b;
                default: r <= 8'hff;
            endcase
        end
    end
    always_comb begin
        if (sel == 2'd0)
            y = r;
        else
            y = ~r;
    end
endmodule
