module Module01 ;
    // integer
    localparam int unsigned a   = 0123456789;
    localparam int unsigned aa  = 01234_56789;

    // binary
    localparam int unsigned b   = 32'b01xzXZ;
    localparam int unsigned bb  = 32'b01_xz_XZ;

    // octal
    localparam int unsigned c   = 32'o01234567xzXZ;
    localparam int unsigned cc  = 32'o01234_567xzXZ;

    // decimal
    localparam int unsigned d   = 32'd0123456789;
    localparam int unsigned dd  = 32'd01234_56789;

    // hex
    localparam int unsigned e   = 128'h0123456789abcdefxzABCDEFXZ;
    localparam int unsigned ee  = 128'h01234_5678_9abc_defxz_ABCD_EFXZ;

    // all0, all1
    localparam int unsigned f   = '0;
    localparam int unsigned ff  = '1;

    // floating point
    localparam int unsigned g      = 0123456789.0123456789;
    localparam int unsigned gg     = 0123456789.0123456789e+0123456789;
    localparam int unsigned ggg    = 0123456789.0123456789e-0123456789;
    localparam int unsigned gggg   = 0123456789.0123456789E+0123456789;
    localparam int unsigned ggggg  = 0123456789.0123456789E-0123456789;
endmodule
