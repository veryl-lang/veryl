module gen_test #(parameter int N = 4) (
    input  logic [N-1:0] in,
    output logic [N-1:0] out
);
    genvar gi;
    generate
        for (gi = 0; gi < N; gi = gi + 1) begin : g_loop
            assign out[gi] = in[gi];
        end
    endgenerate

    generate
        if (N > 2) begin : g_big
            assign out = in;
        end else begin
            assign out = '0;
        end
    endgenerate
endmodule
