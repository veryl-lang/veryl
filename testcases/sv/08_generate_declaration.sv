module veryl_testcase_Module08;
    logic a    ;
    logic b    ;
    logic c    ;
    logic i_clk;

    // if declaration
    if (a == 1) begin :label
        logic a;
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
