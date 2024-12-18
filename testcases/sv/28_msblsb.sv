module veryl_testcase_Module28A (
    input logic [30-1:0][40-1:0] c
);
    localparam int unsigned WIDTH0 = 10;
    localparam int unsigned WIDTH1 = 20;

    logic [10-1:0][20-1:0]              a;
    always_comb a = 1;
    logic [WIDTH0 + 10-1:0][WIDTH1-1:0] b;
    always_comb b = 1;

    logic _x;
    always_comb _x = a[($size(a, 1) - 1)][($size(a, 2) - 1):0 + 1];
    logic _y;
    always_comb _y = b[($size(b, 1) - 1) - 3][($size(b, 2) - 1) + 5:0];
    logic _z;
    always_comb _z = c[($size(c, 1) - 1)][($size(c, 2) - 1)];
endmodule

package veryl_testcase___Package28A__Package28B_B;
    typedef struct packed {
        logic [veryl_testcase_Package28B::B-1:0] a;
    } StructA;
endpackage

package veryl_testcase_Package28B;
    localparam int unsigned B = 2;
endpackage

package veryl_testcase_Package28C;
    localparam int unsigned                W = 2;
    localparam int unsigned                N = 3;
    localparam bit          [N-1:0][W-1:0] C = 0;
endpackage

module veryl_testcase_ModuleB;
    veryl_testcase___Package28A__Package28B_B::StructA a  ;
    always_comb a.a = 0;

    logic _w;
    always_comb _w = a[($bits(a) - 1)];
    logic _x;
    always_comb _x = a.a[($size(a.a, 1) - 1)];
    logic _y;
    always_comb _y = veryl_testcase_Package28C::C[($size(veryl_testcase_Package28C::C, 1) - 1)];
    logic _z;
    always_comb _z = veryl_testcase_Package28C::C[0][($size(veryl_testcase_Package28C::C, 2) - 1)];
endmodule
//# sourceMappingURL=../map/testcases/sv/28_msblsb.sv.map
