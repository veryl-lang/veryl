module ModuleA ;
    // integer
    parameter  int unsigned a   = 0123456789;
    parameter  int unsigned aa  = 01234_56789;

    // binary
    parameter  int unsigned a   = 32'b01xzXZ;
    parameter  int unsigned aa  = 32'b01_xz_XZ;

    // octal
    parameter  int unsigned a   = 32'o01234567xzXZ;
    parameter  int unsigned aa  = 32'o01234_567xzXZ;

    // decimal
    parameter  int unsigned a   = 32'd0123456789;
    parameter  int unsigned aa  = 32'd01234_56789;

    // hex
    parameter  int unsigned a   = 128'h0123456789abcdefxzABCDEFXZ;
    parameter  int unsigned aa  = 128'h01234_5678_9abc_defxz_ABCD_EFXZ;

    // all0, all1
    parameter  int unsigned a   = '0;
    parameter  int unsigned aa  = '1;

    // floating point
    parameter  int unsigned a      = 0123456789.0123456789;
    parameter  int unsigned aa     = 0123456789.0123456789e+0123456789;
    parameter  int unsigned aaa    = 0123456789.0123456789e-0123456789;
    parameter  int unsigned aaaa   = 0123456789.0123456789E+0123456789;
    parameter  int unsigned aaaaa  = 0123456789.0123456789E-0123456789;
endmodule
