module test;
    localparam N = 1000;
    localparam BIT = 32;

    logic           i_clk;
    logic           i_rst;
    logic [BIT-1:0] o_cnt[N];

    counter #(
        .BIT(BIT),
        .N  (N  )
    ) dut (
        .i_clk (i_clk),
        .i_rst (i_rst),
        .o_cnt (o_cnt)
    );

    int i;
    localparam CYCLE = 1000000;

    initial begin
        i_rst = 0;
        i_clk = 0;

        #10;

        i_rst = 1;

        for (i = 0; i < CYCLE * 2; i = i + 1) begin
            #10;
            i_clk = ~i_clk;
        end
        $finish();
    end

    final begin
        for (int i = 0; i < N; i = i + 1) begin
            $display("%d", o_cnt[i]);
        end
    end

endmodule

module counter #(
    parameter BIT = 1,
    parameter N   = 1
)(
    input  var          i_clk,
    input  var          i_rst,
    output var[BIT-1:0] o_cnt[N]
);
    for (genvar i = 0; i < N; i = i + 1) begin: g
        always_ff @ (posedge i_clk or negedge i_rst) begin
            if (~i_rst) begin
                o_cnt[i] <= 0;
            end else begin
                o_cnt[i] <= o_cnt[i] + 1;
            end
        end
    end
endmodule
