module test;

    logic        i_clk;
    logic        i_rst;
    logic [31:0] o_cnt;
    logic [31:0] o_q;

    import "DPI-C" function int accumulator_step(input int d);

    counter dut (
        .i_clk (i_clk),
        .i_rst (i_rst),
        .o_cnt (o_cnt)
    );

    // Reads the pre-edge counter value, exactly as the Veryl component's
    // `on_clock` does; the DPI call holds the accumulator state in C.
    always_ff @(posedge i_clk) begin
        o_q <= accumulator_step(o_cnt);
    end

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
        $display("%d", o_q);
    end

endmodule

module counter (
    input  var        i_clk,
    input  var        i_rst,
    output var [31:0] o_cnt
);
    always_ff @(posedge i_clk or negedge i_rst) begin
        if (~i_rst) begin
            o_cnt <= 0;
        end else begin
            o_cnt <= o_cnt + 1;
        end
    end
endmodule
