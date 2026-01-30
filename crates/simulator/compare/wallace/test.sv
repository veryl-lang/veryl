module test;
    logic        i_clk;
    logic        i_rst;
    logic [63:0] o_out;

    top dut (
        .i_clk (i_clk),
        .i_rst (i_rst),
        .o_out (o_out)
    );

    localparam CYCLE = 1000000;
    int i;

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
        $display("%0d", o_out);
    end

endmodule

module wallace_mul32 (
    input  var [31:0] i_a,
    input  var [31:0] i_b,
    output var [63:0] o_out
);
    // Shifted versions of a: sa[0]=a, sa[1]=a*2, sa[2]=a*4, ...
    logic [63:0] sa [33];

    assign sa[0] = {32'b0, i_a};
    for (genvar i = 0; i < 32; i = i + 1) begin: g_sa
        assign sa[i + 1] = sa[i] + sa[i];
    end

    // Stage 0: Partial products (branchless)
    logic [63:0] mask [32];
    logic [63:0] pp [32];

    always_comb begin
        for (int i = 0; i < 32; i = i + 1) begin
            mask[i] = 64'b0 - {63'b0, i_b[i]};
            pp[i] = sa[i] & mask[i];
        end
    end

    // CSA carry temporaries
    logic [63:0] c1 [10];
    logic [63:0] c2 [7];
    logic [63:0] c3 [5];
    logic [63:0] c4 [3];
    logic [63:0] c5 [2];
    logic [63:0] c6;
    logic [63:0] c7;
    logic [63:0] c8;

    // Stage 1: 32 -> 22 (10 CSAs + 2 pass-through)
    logic [63:0] s1 [22];

    always_comb begin
        for (int i = 0; i < 10; i = i + 1) begin
            s1[i*2]   = pp[i*3] ^ pp[i*3+1] ^ pp[i*3+2];
            c1[i]     = (pp[i*3] & pp[i*3+1]) | (pp[i*3+1] & pp[i*3+2]) | (pp[i*3] & pp[i*3+2]);
            s1[i*2+1] = c1[i] + c1[i];
        end
        s1[20] = pp[30];
        s1[21] = pp[31];
    end

    // Stage 2: 22 -> 15 (7 CSAs + 1 pass-through)
    logic [63:0] s2 [15];

    always_comb begin
        for (int i = 0; i < 7; i = i + 1) begin
            s2[i*2]   = s1[i*3] ^ s1[i*3+1] ^ s1[i*3+2];
            c2[i]     = (s1[i*3] & s1[i*3+1]) | (s1[i*3+1] & s1[i*3+2]) | (s1[i*3] & s1[i*3+2]);
            s2[i*2+1] = c2[i] + c2[i];
        end
        s2[14] = s1[21];
    end

    // Stage 3: 15 -> 10 (5 CSAs)
    logic [63:0] s3 [10];

    always_comb begin
        for (int i = 0; i < 5; i = i + 1) begin
            s3[i*2]   = s2[i*3] ^ s2[i*3+1] ^ s2[i*3+2];
            c3[i]     = (s2[i*3] & s2[i*3+1]) | (s2[i*3+1] & s2[i*3+2]) | (s2[i*3] & s2[i*3+2]);
            s3[i*2+1] = c3[i] + c3[i];
        end
    end

    // Stage 4: 10 -> 7 (3 CSAs + 1 pass-through)
    logic [63:0] s4 [7];

    always_comb begin
        for (int i = 0; i < 3; i = i + 1) begin
            s4[i*2]   = s3[i*3] ^ s3[i*3+1] ^ s3[i*3+2];
            c4[i]     = (s3[i*3] & s3[i*3+1]) | (s3[i*3+1] & s3[i*3+2]) | (s3[i*3] & s3[i*3+2]);
            s4[i*2+1] = c4[i] + c4[i];
        end
        s4[6] = s3[9];
    end

    // Stage 5: 7 -> 5 (2 CSAs + 1 pass-through)
    logic [63:0] s5 [5];

    always_comb begin
        for (int i = 0; i < 2; i = i + 1) begin
            s5[i*2]   = s4[i*3] ^ s4[i*3+1] ^ s4[i*3+2];
            c5[i]     = (s4[i*3] & s4[i*3+1]) | (s4[i*3+1] & s4[i*3+2]) | (s4[i*3] & s4[i*3+2]);
            s5[i*2+1] = c5[i] + c5[i];
        end
        s5[4] = s4[6];
    end

    // Stage 6: 5 -> 4 (1 CSA + 2 pass-through)
    logic [63:0] s6 [4];

    always_comb begin
        s6[0] = s5[0] ^ s5[1] ^ s5[2];
        c6    = (s5[0] & s5[1]) | (s5[1] & s5[2]) | (s5[0] & s5[2]);
        s6[1] = c6 + c6;
        s6[2] = s5[3];
        s6[3] = s5[4];
    end

    // Stage 7: 4 -> 3 (1 CSA + 1 pass-through)
    logic [63:0] s7 [3];

    always_comb begin
        s7[0] = s6[0] ^ s6[1] ^ s6[2];
        c7    = (s6[0] & s6[1]) | (s6[1] & s6[2]) | (s6[0] & s6[2]);
        s7[1] = c7 + c7;
        s7[2] = s6[3];
    end

    // Stage 8: 3 -> 2 (1 CSA)
    logic [63:0] s8 [2];

    always_comb begin
        s8[0] = s7[0] ^ s7[1] ^ s7[2];
        c8    = (s7[0] & s7[1]) | (s7[1] & s7[2]) | (s7[0] & s7[2]);
        s8[1] = c8 + c8;
    end

    // Final addition
    assign o_out = s8[0] + s8[1];

endmodule

module top (
    input  var        i_clk,
    input  var        i_rst,
    output var [63:0] o_out
);
    logic [31:0] a;
    logic [31:0] b;

    always_ff @ (posedge i_clk or negedge i_rst) begin
        if (~i_rst) begin
            a <= 32'h12345678;
            b <= 32'hDEADBEEF;
        end else begin
            a <= a + 1;
            b <= b + a;
        end
    end

    wallace_mul32 u_mul (
        .i_a   (a    ),
        .i_b   (b    ),
        .o_out (o_out)
    );

endmodule
