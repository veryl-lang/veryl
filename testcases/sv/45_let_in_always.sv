module veryl_testcase_Module45;
    logic          a;
    logic [10-1:0] b;
    logic [10-1:0] c;

    always_ff @ (posedge a) begin
        logic [10-1:0] x;
        x = 1;
        b <= x * 1;
    end

    always_comb begin
        logic [10-1:0] y;
        y = 1;
        c = y * 1;
    end
endmodule
