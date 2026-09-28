`define WIDTH 8
module always_bodies (
    input  logic [`WIDTH-1:0] a,
    output logic [`WIDTH-1:0] b,
    output logic [`WIDTH-1:0] c,
    output logic [`WIDTH-1:0] d
);
    always_comb begin
        b = a;
        c = a;
    end
    always_comb // a single statement
        d = a;
endmodule
