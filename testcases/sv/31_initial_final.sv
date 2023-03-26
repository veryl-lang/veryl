module veryl_testcase_Module31 (
    input logic [10-1:0] a
);
    initial begin
        a        <= 1;
        $display("initial");
    end

    final begin
        $display("final");
    end
endmodule
