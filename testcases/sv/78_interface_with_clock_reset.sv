interface veryl_testcase_Interface78;
    logic clk  ;
    logic rst_n;

    modport slave (
        input clk  ,
        input rst_n
    );
endinterface

module veryl_testcase_Module78 (
    veryl_testcase_Interface78.slave intf
);
    logic x;

    always_ff @ (posedge intf.clk, negedge intf.rst_n) begin
        if (!intf.rst_n) begin
            x <= 0;
        end else begin
            x <= x + (1);
        end
    end
endmodule
//# sourceMappingURL=../map/78_interface_with_clock_reset.sv.map
