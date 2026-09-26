module generate_labels #(parameter int N = 4) (
    input  logic [N-1:0] a,
    output logic [N-1:0] b
);
    for (genvar i = 0; i < N; i++) begin
        assign b[i] = a[i];
    end
    if (N == 4) begin : four // a comment after the label
        logic x;
        assign x = a[0];
    end else begin
        logic y;
        assign y = a[0];
    end
    for (genvar j = 0; j < 2; j++) begin : named
        if (1) begin
            logic z;
            assign z = a[j];
        end
    end
endmodule
