module veryl_testcase_Module07;
    logic a  ;
    logic aa ;
    logic clk; always_comb clk = 1;

    always_comb begin
        // assignment statement
        a =    1;
        a +=   1;
        a -=   1;
        a *=   1;
        a /=   1;
        a %=   1;
        a &=   1;
        a |=   1;
        a ^=   1;
        a <<=  1;
        a >>=  1;
        a <<<= 1;
        a >>>= 1;

        // if statement
        if (a) begin
            a  = 1;
            aa = 1;
        end else if (a) begin
            a  = 1;
            aa = 1;
        end else begin
            a  = 1;
            aa = 1;
        end

        // for statement
        for (int unsigned i = 0; i < 10; i++) begin
            a  = i;
            aa = i + 1;
        end

        // for statement with closed range
        for (int unsigned i = 0; i <= 10; i++) begin
            a  = i;
            aa = i + 1;
        end

        // for statement with custom step
        for (int unsigned i = 0; i < 10; i += 2) begin
            a  = i;
            aa = i + 1;
        end
        for (int unsigned i = 0; i < 10; i *= 2) begin
            a  = i;
            aa = i + 1;
        end

        // for statement with break statement
        for (int unsigned i = 0; i < 10; i++) begin
            a  = i;
            aa = i + 1;
            if (i == 0) begin
                break;
            end
        end

        for (int unsigned i = 0; i < 10; i++) begin
            for (int unsigned j = 0; j < 10; j++) begin
                a  = i;
                aa = i + j;
                if (i == 0 && j == 0) begin
                    break;
                end
            end
        end

        for (int unsigned i = 10 - 1; i >= 0; i--) begin
            a  = i;
            aa = i + 1;
            if (i == 9) begin
                break;
            end
        end
    end

    always_ff @ (posedge clk) begin
        a <= a + (1);
        a <= a - (1);
        a <= a * (1);
        a <= a / (1);
        a <= a % (1);
        a <= a & (1);
        a <= a | (1);
        a <= a ^ (1);
        a <= a << (1);
        a <= a >> (1);
        a <= a <<< (1);
        a <= a >>> (1);
    end
endmodule
//# sourceMappingURL=../map/07_statement.sv.map
