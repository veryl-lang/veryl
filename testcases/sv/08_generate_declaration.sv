module ModuleA ;
    // if declaration
    if (a) begin
        always_ff @ (posedge i_clk) begin
            a <= 1;
        end
    end else if (b) begin
        always_ff @ (posedge i_clk) begin
            b <= 1;
        end
    end else begin
        always_ff @ (posedge i_clk) begin
            c <= 1;
        end
    end

    // for declaration
    for (genvar a = 0; a < 10; a++) begin
        always_ff @ (posedge i_clk) begin
            a <= 1;
        end
    end

    // for declaration with custom step
    for (genvar a = 0; a < 10; a += 2) begin
        always_ff @ (posedge i_clk) begin
            a <= 1;
        end
    end
endmodule
