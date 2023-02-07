module veryl_testcase_Module29 (
    input logic clk,
    input logic rst
);
    logic a;
    logic b;
    logic c;
    always_ff @ (posedge clk, negedge rst) begin
        if (!rst) begin
            a <= 0;
        end else begin
            a <= 0;
            b <= 0;
        end
    end
    veryl_testcase_Module29 u0 (


    );
endmodule
