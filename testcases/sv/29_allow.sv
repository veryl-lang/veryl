module veryl_testcase_Module29 (
    input var logic clk  ,
    input var logic rst_n
);
    logic a;
    logic b;

    logic c; always_comb c = 1;

    always_ff @ (posedge clk, negedge rst_n) begin
        if (!rst_n) begin
            a <= 0;
        end else begin
            a <= 0;
            b <= 0;
        end
    end

    veryl_testcase_Module29A u0 ();
endmodule

module veryl_testcase_Module29A (
    input var logic clk  ,
    input var logic rst_n
);
endmodule
//# sourceMappingURL=../map/testcases/sv/29_allow.sv.map
