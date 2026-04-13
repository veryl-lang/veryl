module foo #(parameter WIDTH = 8) (
    input  logic [WIDTH-1:0] a,
    input  logic             clk,
    input  logic             rst,
    output logic [WIDTH-1:0] q
);
    logic [WIDTH-1:0] r;
    assign q = r;
    always_ff @(posedge clk or negedge rst) begin
        if (!rst) r <= '0;
        else      r <= a;
    end
    bar u_bar (.x(a), .y(q));
endmodule
