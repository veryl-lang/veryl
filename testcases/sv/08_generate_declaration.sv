module ModuleA ;
    // if declaration
    if (a == 1) begin :label
        always_ff @ (posedge i_clk) begin
            a <= 1;
        end
    end else if (b == 1) begin :label // label can be omit in else clause
        always_ff @ (posedge i_clk) begin
            b <= 1;
        end
    end else if (b == 1) begin :label1 // label can be override in the specified clause only
        always_ff @ (posedge i_clk) begin
            b <= 1;
        end
    end else begin :label
        always_ff @ (posedge i_clk) begin
            c <= 1;
        end
    end

    // for declaration
    for (genvar a = 0; a < 10; a++) begin :label
        always_ff @ (posedge i_clk) begin
            a <= 1;
        end
    end

    // for declaration with custom step
    for (genvar a = 0; a < 10; a += 2) begin :label
        always_ff @ (posedge i_clk) begin
            a <= 1;
        end
    end
endmodule
