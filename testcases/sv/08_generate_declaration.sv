module veryl_testcase_Module08;
    localparam int unsigned a     = 1;
    localparam int unsigned b     = 1;
    logic        i_clk; always_comb i_clk = 1;

    // if declaration
    if (a == 1) begin :label
        logic a;
        always_ff @ (posedge i_clk) begin
            a <= 1;
        end
    end else if (b == 1) begin :label // label can be omit in else clause
        logic a;
        always_ff @ (posedge i_clk) begin
            a <= 1;
        end
    end else if (b == 1) begin :label1 // label can be override in the specified clause only
        logic a;
        always_ff @ (posedge i_clk) begin
            a <= 1;
        end
    end else begin :label
        logic a;
        always_ff @ (posedge i_clk) begin
            a <= 1;
        end
    end

    // for declaration
    for (genvar i = 0; i < 10; i++) begin :label2
        logic a;
        always_ff @ (posedge i_clk) begin
            a <= i;
        end
    end

    // for declaration with custom step
    for (genvar i = 0; i < 10; i += 2) begin :label3
        logic a;
        always_ff @ (posedge i_clk) begin
            a <= i;
        end
    end
endmodule
//# sourceMappingURL=../map/08_generate_declaration.sv.map
