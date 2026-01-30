module test;
    logic        i_clk;
    logic        i_rst;
    logic [31:0] o_out;

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

module riscv_core (
    input  var        i_clk,
    input  var        i_rst,
    output var [31:0] o_out
);
    // ==================== Memory ====================
    logic [31:0] imem [256];
    logic [31:0] dmem [256];
    logic [31:0] regfile [32];

    initial begin
        $readmemh("program.hex", imem);
    end

    // ==================== Pipeline Registers ====================
    logic [31:0] pc;

    // IF/ID
    logic [31:0] if_id_pc;
    logic [31:0] if_id_instr;
    logic        if_id_valid;

    // ID/EX
    logic [31:0] id_ex_pc;
    logic [31:0] id_ex_rs1_data;
    logic [31:0] id_ex_rs2_data;
    logic [31:0] id_ex_imm;
    logic [4:0]  id_ex_rd;
    logic [4:0]  id_ex_rs1;
    logic [4:0]  id_ex_rs2;
    logic [3:0]  id_ex_alu_op;
    logic        id_ex_alu_src;
    logic        id_ex_mem_read;
    logic        id_ex_mem_write;
    logic        id_ex_reg_write;
    logic [1:0]  id_ex_wb_sel;
    logic        id_ex_branch;
    logic [2:0]  id_ex_funct3;
    logic        id_ex_jump;
    logic        id_ex_is_jalr;
    logic        id_ex_imm_result;
    logic        id_ex_valid;

    // EX/MEM
    logic [31:0] ex_mem_alu_result;
    logic [31:0] ex_mem_rs2_data;
    logic [4:0]  ex_mem_rd;
    logic [31:0] ex_mem_pc_plus4;
    logic        ex_mem_mem_read;
    logic        ex_mem_mem_write;
    logic        ex_mem_reg_write;
    logic [1:0]  ex_mem_wb_sel;
    logic        ex_mem_valid;

    // MEM/WB
    logic [31:0] mem_wb_result;
    logic [4:0]  mem_wb_rd;
    logic        mem_wb_reg_write;
    logic        mem_wb_valid;

    // ==================== Decode ====================
    logic [6:0]  dec_opcode;
    logic [4:0]  dec_rd;
    logic [2:0]  dec_funct3;
    logic [4:0]  dec_rs1;
    logic [4:0]  dec_rs2;
    logic [6:0]  dec_funct7;

    logic [31:0] dec_imm_i;
    logic [31:0] dec_imm_s;
    logic [31:0] dec_imm_b;
    logic [31:0] dec_imm_u;
    logic [31:0] dec_imm_j;

    logic [31:0] dec_imm;
    logic [3:0]  dec_alu_op;
    logic        dec_alu_src;
    logic        dec_mem_read;
    logic        dec_mem_write;
    logic        dec_reg_write;
    logic [1:0]  dec_wb_sel;
    logic        dec_branch;
    logic        dec_jump;
    logic        dec_is_jalr;
    logic        dec_imm_result;
    logic [4:0]  dec_rs1_addr;
    logic [4:0]  dec_rs2_addr;

    always_comb begin
        dec_opcode = if_id_instr[6:0];
        dec_rd     = if_id_instr[11:7];
        dec_funct3 = if_id_instr[14:12];
        dec_rs1    = if_id_instr[19:15];
        dec_rs2    = if_id_instr[24:20];
        dec_funct7 = if_id_instr[31:25];

        dec_imm_i = {{20{if_id_instr[31]}}, if_id_instr[31:20]};
        dec_imm_s = {{20{if_id_instr[31]}}, if_id_instr[31:25], if_id_instr[11:7]};
        dec_imm_b = {{20{if_id_instr[31]}}, if_id_instr[7], if_id_instr[30:25], if_id_instr[11:8], 1'b0};
        dec_imm_u = {if_id_instr[31:12], 12'b0};
        dec_imm_j = {{12{if_id_instr[31]}}, if_id_instr[19:12], if_id_instr[20], if_id_instr[30:21], 1'b0};

        dec_alu_op     = 4'b0000;
        dec_alu_src    = 0;
        dec_mem_read   = 0;
        dec_mem_write  = 0;
        dec_reg_write  = 0;
        dec_wb_sel     = 2'b00;
        dec_branch     = 0;
        dec_jump       = 0;
        dec_is_jalr    = 0;
        dec_imm_result = 0;
        dec_imm        = 32'b0;
        dec_rs1_addr   = dec_rs1;
        dec_rs2_addr   = dec_rs2;

        if (if_id_valid) begin
            case (dec_opcode)
                7'b0110011: begin
                    dec_alu_op    = {dec_funct7[5], dec_funct3};
                    dec_reg_write = 1;
                end
                7'b0010011: begin
                    dec_alu_op    = (dec_funct3 == 3'b101) ? {dec_funct7[5], dec_funct3} : {1'b0, dec_funct3};
                    dec_alu_src   = 1;
                    dec_reg_write = 1;
                    dec_imm       = dec_imm_i;
                end
                7'b0000011: begin
                    dec_alu_op    = 4'b0000;
                    dec_alu_src   = 1;
                    dec_mem_read  = 1;
                    dec_reg_write = 1;
                    dec_wb_sel    = 2'b01;
                    dec_imm       = dec_imm_i;
                end
                7'b0100011: begin
                    dec_alu_op    = 4'b0000;
                    dec_alu_src   = 1;
                    dec_mem_write = 1;
                    dec_imm       = dec_imm_s;
                end
                7'b1100011: begin
                    dec_branch = 1;
                    dec_imm    = dec_imm_b;
                end
                7'b0110111: begin
                    dec_reg_write  = 1;
                    dec_imm        = dec_imm_u;
                    dec_imm_result = 1;
                    dec_rs1_addr   = 5'b0;
                    dec_rs2_addr   = 5'b0;
                end
                7'b0010111: begin
                    dec_reg_write  = 1;
                    dec_imm        = if_id_pc + dec_imm_u;
                    dec_imm_result = 1;
                    dec_rs1_addr   = 5'b0;
                    dec_rs2_addr   = 5'b0;
                end
                7'b1101111: begin
                    dec_jump      = 1;
                    dec_reg_write = 1;
                    dec_wb_sel    = 2'b10;
                    dec_imm       = dec_imm_j;
                    dec_rs1_addr  = 5'b0;
                    dec_rs2_addr  = 5'b0;
                end
                7'b1100111: begin
                    dec_jump      = 1;
                    dec_is_jalr   = 1;
                    dec_reg_write = 1;
                    dec_wb_sel    = 2'b10;
                    dec_alu_src   = 1;
                    dec_imm       = dec_imm_i;
                end
                default: ;
            endcase
        end
    end

    // ==================== Register File Read ====================
    logic [31:0] rf_rs1_data;
    logic [31:0] rf_rs2_data;

    always_comb begin
        rf_rs1_data =
            (dec_rs1_addr == 5'b0) ? 32'b0 :
            (mem_wb_valid && mem_wb_reg_write && mem_wb_rd == dec_rs1_addr) ? mem_wb_result :
            regfile[dec_rs1_addr];

        rf_rs2_data =
            (dec_rs2_addr == 5'b0) ? 32'b0 :
            (mem_wb_valid && mem_wb_reg_write && mem_wb_rd == dec_rs2_addr) ? mem_wb_result :
            regfile[dec_rs2_addr];
    end

    // ==================== Hazard Detection ====================
    logic stall;

    always_comb begin
        stall = id_ex_valid && id_ex_mem_read && if_id_valid && (
            (dec_rs1_addr == id_ex_rd && id_ex_rd != 5'b0) ||
            (dec_rs2_addr == id_ex_rd && id_ex_rd != 5'b0)
        );
    end

    // ==================== Forwarding ====================
    logic [31:0] fwd_rs1;
    logic [31:0] fwd_rs2;
    logic [31:0] ex_mem_fwd_data;

    always_comb begin
        ex_mem_fwd_data = (ex_mem_wb_sel == 2'b10) ? ex_mem_pc_plus4 : ex_mem_alu_result;

        fwd_rs1 =
            (id_ex_rs1 == 5'b0) ? 32'b0 :
            (ex_mem_valid && ex_mem_reg_write && ex_mem_rd == id_ex_rs1) ? ex_mem_fwd_data :
            (mem_wb_valid && mem_wb_reg_write && mem_wb_rd == id_ex_rs1) ? mem_wb_result :
            id_ex_rs1_data;

        fwd_rs2 =
            (id_ex_rs2 == 5'b0) ? 32'b0 :
            (ex_mem_valid && ex_mem_reg_write && ex_mem_rd == id_ex_rs2) ? ex_mem_fwd_data :
            (mem_wb_valid && mem_wb_reg_write && mem_wb_rd == id_ex_rs2) ? mem_wb_result :
            id_ex_rs2_data;
    end

    // ==================== ALU ====================
    logic [31:0] alu_in1;
    logic [31:0] alu_in2;
    logic [31:0] alu_result;
    logic [31:0] alu_add;
    logic [31:0] alu_sub;
    logic        alu_slt;
    logic        alu_sltu;
    logic [4:0]  shamt;

    logic [31:0] sll_s0, sll_s1, sll_s2, sll_s3, sll_s4;
    logic [31:0] srl_s0, srl_s1, srl_s2, srl_s3, srl_s4;
    logic [31:0] sra_s0, sra_s1, sra_s2, sra_s3, sra_s4;

    always_comb begin
        alu_in1 = fwd_rs1;
        alu_in2 = id_ex_alu_src ? id_ex_imm : fwd_rs2;

        alu_add = alu_in1 + alu_in2;
        alu_sub = alu_in1 - alu_in2;

        alu_slt  = (alu_in1[31] & ~alu_in2[31]) | (~(alu_in1[31] ^ alu_in2[31]) & alu_sub[31]);
        alu_sltu = (alu_in1 < alu_in2);

        shamt  = alu_in2[4:0];
        sll_s0 = shamt[0] ? {alu_in1[30:0], 1'b0}  : alu_in1;
        sll_s1 = shamt[1] ? {sll_s0[29:0], 2'b0}    : sll_s0;
        sll_s2 = shamt[2] ? {sll_s1[27:0], 4'b0}    : sll_s1;
        sll_s3 = shamt[3] ? {sll_s2[23:0], 8'b0}    : sll_s2;
        sll_s4 = shamt[4] ? {sll_s3[15:0], 16'b0}   : sll_s3;

        srl_s0 = shamt[0] ? {1'b0, alu_in1[31:1]}   : alu_in1;
        srl_s1 = shamt[1] ? {2'b0, srl_s0[31:2]}    : srl_s0;
        srl_s2 = shamt[2] ? {4'b0, srl_s1[31:4]}    : srl_s1;
        srl_s3 = shamt[3] ? {8'b0, srl_s2[31:8]}    : srl_s2;
        srl_s4 = shamt[4] ? {16'b0, srl_s3[31:16]}  : srl_s3;

        sra_s0 = shamt[0] ? {alu_in1[31], alu_in1[31:1]}          : alu_in1;
        sra_s1 = shamt[1] ? {{2{alu_in1[31]}}, sra_s0[31:2]}     : sra_s0;
        sra_s2 = shamt[2] ? {{4{alu_in1[31]}}, sra_s1[31:4]}     : sra_s1;
        sra_s3 = shamt[3] ? {{8{alu_in1[31]}}, sra_s2[31:8]}     : sra_s2;
        sra_s4 = shamt[4] ? {{16{alu_in1[31]}}, sra_s3[31:16]}   : sra_s3;

        if (id_ex_imm_result) begin
            alu_result = id_ex_imm;
        end else begin
            case (id_ex_alu_op)
                4'b0000: alu_result = alu_add;
                4'b1000: alu_result = alu_sub;
                4'b0001: alu_result = sll_s4;
                4'b0010: alu_result = {31'b0, alu_slt};
                4'b0011: alu_result = {31'b0, alu_sltu};
                4'b0100: alu_result = alu_in1 ^ alu_in2;
                4'b0101: alu_result = srl_s4;
                4'b1101: alu_result = sra_s4;
                4'b0110: alu_result = alu_in1 | alu_in2;
                4'b0111: alu_result = alu_in1 & alu_in2;
                default: alu_result = alu_add;
            endcase
        end
    end

    // ==================== Branch Resolution ====================
    logic        branch_taken;
    logic [31:0] branch_target;
    logic        flush;
    logic [31:0] branch_sub;

    always_comb begin
        branch_taken  = 0;
        branch_target = 32'b0;
        branch_sub    = fwd_rs1 - fwd_rs2;

        if (id_ex_valid && id_ex_jump) begin
            branch_taken = 1;
            if (id_ex_is_jalr)
                branch_target = (fwd_rs1 + id_ex_imm) & 32'hFFFFFFFE;
            else
                branch_target = id_ex_pc + id_ex_imm;
        end else if (id_ex_valid && id_ex_branch) begin
            branch_target = id_ex_pc + id_ex_imm;
            case (id_ex_funct3)
                3'b000: branch_taken = (branch_sub == 32'b0);
                3'b001: branch_taken = (branch_sub != 32'b0);
                3'b100: branch_taken = (fwd_rs1[31] & ~fwd_rs2[31])
                                     | (~(fwd_rs1[31] ^ fwd_rs2[31]) & branch_sub[31]);
                3'b101: branch_taken = ~((fwd_rs1[31] & ~fwd_rs2[31])
                                     | (~(fwd_rs1[31] ^ fwd_rs2[31]) & branch_sub[31]));
                3'b110: branch_taken = (fwd_rs1 < fwd_rs2);
                3'b111: branch_taken = ~(fwd_rs1 < fwd_rs2) | (branch_sub == 32'b0);
                default: ;
            endcase
        end

        flush = branch_taken;
    end

    // ==================== MEM / WB Data ====================
    logic [31:0] mem_read_data;
    logic [31:0] wb_data;

    always_comb begin
        mem_read_data = dmem[ex_mem_alu_result[9:2]];
    end

    always_comb begin
        case (ex_mem_wb_sel)
            2'b01:   wb_data = mem_read_data;
            2'b10:   wb_data = ex_mem_pc_plus4;
            default: wb_data = ex_mem_alu_result;
        endcase
    end

    // ==================== Sequential: PC ====================
    always_ff @(posedge i_clk or negedge i_rst) begin
        if (~i_rst)
            pc <= 32'b0;
        else if (flush)
            pc <= branch_target;
        else if (~stall)
            pc <= pc + 4;
    end

    // ==================== Sequential: IF/ID ====================
    always_ff @(posedge i_clk or negedge i_rst) begin
        if (~i_rst) begin
            if_id_valid <= 0;
            if_id_pc    <= 32'b0;
            if_id_instr <= 32'b0;
        end else if (flush) begin
            if_id_valid <= 0;
            if_id_pc    <= 32'b0;
            if_id_instr <= 32'b0;
        end else if (~stall) begin
            if_id_valid <= 1;
            if_id_pc    <= pc;
            if_id_instr <= imem[pc[9:2]];
        end
    end

    // ==================== Sequential: ID/EX ====================
    always_ff @(posedge i_clk or negedge i_rst) begin
        if (~i_rst) begin
            id_ex_valid      <= 0;
            id_ex_reg_write  <= 0;
            id_ex_mem_read   <= 0;
            id_ex_mem_write  <= 0;
            id_ex_branch     <= 0;
            id_ex_jump       <= 0;
            id_ex_pc         <= 32'b0;
            id_ex_rs1_data   <= 32'b0;
            id_ex_rs2_data   <= 32'b0;
            id_ex_imm        <= 32'b0;
            id_ex_rd         <= 5'b0;
            id_ex_rs1        <= 5'b0;
            id_ex_rs2        <= 5'b0;
            id_ex_alu_op     <= 4'b0;
            id_ex_alu_src    <= 0;
            id_ex_wb_sel     <= 2'b0;
            id_ex_funct3     <= 3'b0;
            id_ex_is_jalr    <= 0;
            id_ex_imm_result <= 0;
        end else if (flush || stall) begin
            id_ex_valid      <= 0;
            id_ex_reg_write  <= 0;
            id_ex_mem_read   <= 0;
            id_ex_mem_write  <= 0;
            id_ex_branch     <= 0;
            id_ex_jump       <= 0;
        end else begin
            id_ex_valid      <= if_id_valid;
            id_ex_pc         <= if_id_pc;
            id_ex_rs1_data   <= rf_rs1_data;
            id_ex_rs2_data   <= rf_rs2_data;
            id_ex_imm        <= dec_imm;
            id_ex_rd         <= dec_rd;
            id_ex_rs1        <= dec_rs1_addr;
            id_ex_rs2        <= dec_rs2_addr;
            id_ex_alu_op     <= dec_alu_op;
            id_ex_alu_src    <= dec_alu_src;
            id_ex_mem_read   <= dec_mem_read;
            id_ex_mem_write  <= dec_mem_write;
            id_ex_reg_write  <= dec_reg_write;
            id_ex_wb_sel     <= dec_wb_sel;
            id_ex_branch     <= dec_branch;
            id_ex_funct3     <= dec_funct3;
            id_ex_jump       <= dec_jump;
            id_ex_is_jalr    <= dec_is_jalr;
            id_ex_imm_result <= dec_imm_result;
        end
    end

    // ==================== Sequential: EX/MEM ====================
    always_ff @(posedge i_clk or negedge i_rst) begin
        if (~i_rst) begin
            ex_mem_valid      <= 0;
            ex_mem_reg_write  <= 0;
            ex_mem_mem_read   <= 0;
            ex_mem_mem_write  <= 0;
            ex_mem_alu_result <= 32'b0;
            ex_mem_rs2_data   <= 32'b0;
            ex_mem_rd         <= 5'b0;
            ex_mem_pc_plus4   <= 32'b0;
            ex_mem_wb_sel     <= 2'b0;
        end else begin
            ex_mem_valid      <= id_ex_valid;
            ex_mem_alu_result <= alu_result;
            ex_mem_rs2_data   <= fwd_rs2;
            ex_mem_rd         <= id_ex_rd;
            ex_mem_pc_plus4   <= id_ex_pc + 4;
            ex_mem_mem_read   <= id_ex_mem_read;
            ex_mem_mem_write  <= id_ex_mem_write;
            ex_mem_reg_write  <= id_ex_reg_write;
            ex_mem_wb_sel     <= id_ex_wb_sel;
        end
    end

    // ==================== Sequential: MEM/WB ====================
    always_ff @(posedge i_clk or negedge i_rst) begin
        if (~i_rst) begin
            mem_wb_valid     <= 0;
            mem_wb_reg_write <= 0;
            mem_wb_result    <= 32'b0;
            mem_wb_rd        <= 5'b0;
        end else begin
            mem_wb_valid     <= ex_mem_valid;
            mem_wb_result    <= wb_data;
            mem_wb_rd        <= ex_mem_rd;
            mem_wb_reg_write <= ex_mem_reg_write;
        end
    end

    // ==================== Data Memory Write ====================
    always_ff @(posedge i_clk) begin
        if (ex_mem_valid && ex_mem_mem_write) begin
            dmem[ex_mem_alu_result[9:2]] <= ex_mem_rs2_data;
        end
    end

    // ==================== Register File Write ====================
    always_ff @(posedge i_clk) begin
        if (mem_wb_valid && mem_wb_reg_write && mem_wb_rd != 5'b0) begin
            regfile[mem_wb_rd] <= mem_wb_result;
        end
    end

    assign o_out = mem_wb_result;

endmodule

module top (
    input  var        i_clk,
    input  var        i_rst,
    output var [31:0] o_out
);
    riscv_core u_core (
        .i_clk (i_clk),
        .i_rst (i_rst),
        .o_out (o_out)
    );
endmodule
