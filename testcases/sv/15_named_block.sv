module veryl_testcase_Module15;
    logic _a; always_comb _a = 1;

    if (1) begin :label
        logic _a; always_comb _a = 1;
    end

    if (1) begin :label1
        // a comment at the top of the block
        logic _a; always_comb _a = 1;
    end

    for (genvar i = 0; i < 10; i++) begin :label2
        // the same in a loop body
        if (1) begin :label
            logic _a; always_comb _a = 1;
        end
    end
endmodule
//# sourceMappingURL=../map/15_named_block.sv.map
