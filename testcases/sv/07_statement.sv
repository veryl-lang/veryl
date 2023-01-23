module veryl_testcase_Module07;
    logic  a  ;
    logic  aa ;
    logic  clk;

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
        for (int unsigned i  = 0; i < 10; i++) begin
            a  = i;
            aa = i + 1;
        end

        // for statement with custom step
        for (int unsigned i  = 0; i < 10; i += 2) begin
            a  = i;
            aa = i + 1;
        end
        for (int unsigned i  = 0; i < 10; i *= 2) begin
            a  = i;
            aa = i + 1;
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
