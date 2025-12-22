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

        for (int signed i = 10 - 1; i >= 0; i--) begin
            a  = i;
            aa = i + 1;
            if (i == 9) begin
                break;
            end
        end
    end

    logic a00;
    logic a01;
    logic a02;
    logic a03;
    logic a04;
    logic a05;
    logic a06;
    logic a07;
    logic a08;
    logic a09;
    logic a10;
    logic a11;
    always_ff @ (posedge clk) begin
        a00 <= a00 + (1);
        a01 <= a01 - (1);
        a02 <= a02 * (1);
        a03 <= a03 / (1);
        a04 <= a04 % (1);
        a05 <= a05 & (1);
        a06 <= a06 | (1);
        a07 <= a07 ^ (1);
        a08 <= a08 << (1);
        a09 <= a09 >> (1);
        a10 <= a10 <<< (1);
        a11 <= a11 >>> (1);
    end
endmodule
//# sourceMappingURL=../map/07_statement.sv.map
