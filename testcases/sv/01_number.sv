module ModuleA ;
    // integer
    parameter  int unsigned a   = 0123456789;
    parameter  int unsigned aa  = 01234_56789;

    // binary
    parameter  int unsigned b   = 32'b01xzXZ;
    parameter  int unsigned bb  = 32'b01_xz_XZ;

    // octal
    parameter  int unsigned c   = 32'o01234567xzXZ;
    parameter  int unsigned cc  = 32'o01234_567xzXZ;

    // decimal
    parameter  int unsigned d   = 32'd0123456789;
    parameter  int unsigned dd  = 32'd01234_56789;

    // hex
    parameter  int unsigned e   = 128'h0123456789abcdefxzABCDEFXZ;
    parameter  int unsigned ee  = 128'h01234_5678_9abc_defxz_ABCD_EFXZ;

    // all0, all1
    parameter  int unsigned f   = '0;
    parameter  int unsigned ff  = '1;

    // floating point
    parameter  int unsigned g      = 0123456789.0123456789;
    parameter  int unsigned gg     = 0123456789.0123456789e+0123456789;
    parameter  int unsigned ggg    = 0123456789.0123456789e-0123456789;
    parameter  int unsigned gggg   = 0123456789.0123456789E+0123456789;
    parameter  int unsigned ggggg  = 0123456789.0123456789E-0123456789;
endmodule
