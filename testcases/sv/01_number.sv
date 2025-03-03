module veryl_testcase_Module01;
    // integer
    localparam int unsigned a  = 0123456789;
    localparam int unsigned aa = 01234_56789;

    // binary
    localparam logic [32-1:0] b   = 32'b01xzXZ;
    localparam logic [32-1:0] bb  = 32'b01_xz_XZ;
    localparam logic [32-1:0] bbb = 32'sb01_xz_XZ;

    // octal
    localparam logic [32-1:0] c   = 32'o01234567xzXZ;
    localparam logic [32-1:0] cc  = 32'o01234_567xzXZ;
    localparam logic [32-1:0] ccc = 32'so01234_567xzXZ;

    // decimal
    localparam int unsigned d   = 32'd0123456789;
    localparam int unsigned dd  = 32'd01234_56789;
    localparam int unsigned ddd = 32'sd01234_56789;

    // hex
    localparam logic [128-1:0] e   = 128'h0123456789abcdefxzABCDEFXZ;
    localparam logic [128-1:0] ee  = 128'h01234_5678_9abc_defxz_ABCD_EFXZ;
    localparam logic [128-1:0] eee = 128'sh01234_5678_9abc_defxz_ABCD_EFXZ;

    // all0, all1, allx, allz
    localparam logic [32-1:0] f      = '0;
    localparam logic [32-1:0] ff     = '1;
    localparam logic [32-1:0] fff    = 'x;
    localparam logic [32-1:0] ffff   = 'X;
    localparam logic [32-1:0] fffff  = 'z;
    localparam logic [32-1:0] ffffff = 'Z;

    // floating point
    localparam int unsigned g     = 0123456789.0123456789;
    localparam int unsigned gg    = 0123456789.0123456789e+0123456789;
    localparam int unsigned ggg   = 0123456789.0123456789e-0123456789;
    localparam int unsigned gggg  = 0123456789.0123456789E+0123456789;
    localparam int unsigned ggggg = 0123456789.0123456789E-0123456789;

    // width-less based
    localparam logic [32-1:0] h   = 1'b0;
    localparam logic [32-1:0] hh  = 1'o0;
    localparam logic [32-1:0] hhh = 1'h0;
endmodule
//# sourceMappingURL=../map/testcases/sv/01_number.sv.map
