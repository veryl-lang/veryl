module test;

    logic           i_clk;
    logic           i_rst;
    logic [255:0]   o_q;

    xorshift256 dut (
        .i_clk (i_clk),
        .i_rst (i_rst),
        .o_q   (o_q  )
    );

    localparam CYCLE = 1000000;

    initial begin
        i_rst = 0;
        i_clk = 0;

        #10;

        i_rst = 1;

        for (int i = 0; i < CYCLE * 2; i = i + 1) begin
            #10;
            i_clk = ~i_clk;
        end
        $finish();
    end

    final begin
        $display("%h", o_q);
    end

endmodule

module xorshift256 (
    input  var          i_clk,
    input  var          i_rst,
    output var [255:0]  o_q
);
    logic [255:0] state;
    logic [255:0] t;

    assign o_q = state;

    always_ff @(posedge i_clk or negedge i_rst) begin
        if (~i_rst) begin
            state <= 256'd1;
        end else begin
            t = state ^ (state << 13);
            t = t ^ (t >> 7);
            state <= t ^ (t << 17);
        end
    end
endmodule
