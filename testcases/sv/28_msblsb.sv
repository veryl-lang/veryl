module veryl_testcase_Module28 (
    input logic [30-1:0][40-1:0] c
);
    localparam int unsigned WIDTH0 = 10;
    localparam int unsigned WIDTH1 = 20;

    logic [10-1:0][20-1:0]              a;
    always_comb a = 1;
    logic [WIDTH0 + 10-1:0][WIDTH1-1:0] b;
    always_comb b = 1;

    logic _x;
    always_comb _x = a[((10) - 1)][((20) - 1):0 + 1];
    logic _y;
    always_comb _y = b[((WIDTH0 + 10) - 1) - 3][((WIDTH1) - 1) + 5:0];
    logic _z;
    always_comb _z = c[((30) - 1)][((40) - 1)];
endmodule
//# sourceMappingURL=../map/testcases/sv/28_msblsb.sv.map
