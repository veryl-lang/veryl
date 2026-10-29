module veryl_testcase_Module92 (
    input  var logic          i_clk0 ,
    input  var logic          i_clk1 ,
    input  var logic          i_en0  ,
    input  var logic          i_en1  ,
    input  var logic [5-1:0]  i_addr0,
    input  var logic [5-1:0]  i_addr1,
    input  var logic [32-1:0] i_data0,
    input  var logic [32-1:0] i_data1,
    output var logic [32-1:0] o_data ,
    output var logic [16-1:0] o_count,
    output var logic [32-1:0] o_rom  
);
    // True dual port SRAM inference on FPGA needs two processes writing one
    // variable, which SystemVerilog forbids for always_ff, so both are emitted
    // as plain always.

    logic [32-1:0] ram [32];

    always @ (posedge i_clk0) begin
        if (i_en0) begin
            ram[i_addr0] <= i_data0;
        end
    end

    always @ (posedge i_clk1) begin
        if (i_en1) begin
            ram[i_addr1] <= i_data1;
        end
    end

    always_comb o_data = ram[i_addr0];

    // Initialized by the configuration bitstream instead of by a reset.

    logic [16-1:0] count;

    logic [32-1:0] rom [1024];

    initial begin
        count = '0;
        $readmemh("rom.hex", rom);
    end

    always @ (posedge i_clk0) begin
        count <= count + 1;
    end

    always_comb o_count = count;
    always_comb o_rom   = rom[i_addr0];
endmodule
//# sourceMappingURL=../map/92_non_portable.sv.map
