module veryl_testcase_Module15;
    logic _a;
    always_comb _a = 1;
    if (1) begin 
    :label
        logic _a;
        always_comb _a = 1;
    end
    if (1) begin 
    :label1
        logic _a;
        always_comb _a = 1;
    end
endmodule
