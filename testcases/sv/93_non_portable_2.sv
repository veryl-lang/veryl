module veryl_testcase_Module93 (
    input  var logic                   i_clk  ,
    input  var logic                   i_rst_n,
    input  var logic [4-1:0]           i_wen0 ,
    input  var logic [4-1:0][8-1:0]    i_din0 ,
    input  var logic [$clog2(128)-1:0] i_addr0,
    output var logic [4-1:0][8-1:0]    o_dout0,
    input  var logic [4-1:0]           i_wen1 ,
    input  var logic [4-1:0][8-1:0]    i_din1 ,
    input  var logic [$clog2(128)-1:0] i_addr1,
    output var logic [4-1:0][8-1:0]    o_dout1
);
    `ifdef GOWIN_SYNTH
    for (genvar b = 0; b < 4; b++) begin :BYTE_MEMORY

        logic [8-1:0] memory [128];

        always @ (posedge i_clk) begin
            if ((i_wen0[b])) begin
                memory[i_addr0] <= i_din0[b];
            end
            o_dout0[b] <= memory[i_addr0];
        end
        always @ (posedge i_clk) begin
            if ((i_wen1[b])) begin
                memory[i_addr1] <= i_din1[b];
            end
            o_dout1[b] <= memory[i_addr1];
        end
    end

    `else
    logic [(4 * 8)-1:0] memory [128];

    always @ (posedge i_clk) begin
        for (int b = 0; b < 4; b++) begin
            if ((i_wen0[b])) begin
                memory[i_addr0][b * 8+:8] <= i_din0[b];
            end
        end
        o_dout0 <= memory[i_addr0];
    end
    always @ (posedge i_clk) begin
        for (int b = 0; b < 4; b++) begin
            if ((i_wen1[b])) begin
                memory[i_addr1][b * 8+:8] <= i_din1[b];
            end
        end
        o_dout1 <= memory[i_addr1];
    end
    `endif

endmodule
//# sourceMappingURL=../map/93_non_portable_2.sv.map
