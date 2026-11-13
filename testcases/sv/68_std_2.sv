module veryl_testcase_Module68H
    import veryl_testcase_Package68G::WIDTH;
#(
    localparam int unsigned RESULT_WIDTH = __std_lzc_pkg::result_width(WIDTH)
) (
    input  var logic [WIDTH-1:0]        i_d     ,
    output var logic [RESULT_WIDTH-1:0] o_result
);


    always_comb o_result = __std___lzc__Package68G_WIDTH(i_d, 1'b1);

    function automatic logic [veryl_testcase_Package68G::WIDTH-1:0] __std___bit_reverse__Package68G_WIDTH(
        input var logic [veryl_testcase_Package68G::WIDTH-1:0] bits
    ) ;
        logic [veryl_testcase_Package68G::WIDTH-1:0] ret;

        for (int i = 0; i < veryl_testcase_Package68G::WIDTH; i++) begin
            ret[i] = bits[veryl_testcase_Package68G::WIDTH - i - 1];
        end

        return ret;
    endfunction
    function automatic logic [__std_lzc_pkg::result_width(
        veryl_testcase_Package68G::WIDTH
    )-1:0] __std___lzc__Package68G_WIDTH(
        input var logic [veryl_testcase_Package68G::WIDTH-1:0] bits    , /// Input vector
        input var logic                                        from_msb /// Counting direction (true: from MSB, false: from LSB)
    ) ;
        localparam int unsigned RESULT_WIDTH = __std_lzc_pkg::result_width(veryl_testcase_Package68G::WIDTH);
        localparam int unsigned COUNT_WIDTH  = RESULT_WIDTH - 1;
        localparam int unsigned N_SPLITS     = (veryl_testcase_Package68G::WIDTH + 63) / 64;
        logic [veryl_testcase_Package68G::WIDTH-1:0] input_bits;
        logic [RESULT_WIDTH-1:0]                     ret       ;

        if (from_msb) begin
            input_bits = __std___bit_reverse__Package68G_WIDTH(bits);
        end else begin
            input_bits = bits;
        end

        if (veryl_testcase_Package68G::WIDTH <= 4) begin
            __std_lzc_pkg::__result__4 ret4;
            ret4 = __std_lzc_pkg::lzc4(4'(input_bits));
            ret  = {ret4.all_zero, COUNT_WIDTH'(ret4.count)};
        end else if (veryl_testcase_Package68G::WIDTH <= 8) begin
            __std_lzc_pkg::__result__8 ret8;
            ret8 = __std_lzc_pkg::lzc8(8'(input_bits));
            ret  = {ret8.all_zero, COUNT_WIDTH'(ret8.count)};
        end else if (veryl_testcase_Package68G::WIDTH <= 16) begin
            __std_lzc_pkg::__result__16 ret16;
            ret16 = __std_lzc_pkg::lzc16(16'(input_bits));
            ret   = {ret16.all_zero, COUNT_WIDTH'(ret16.count)};
        end else if (veryl_testcase_Package68G::WIDTH <= 32) begin
            __std_lzc_pkg::__result__32 ret32;
            ret32 = __std_lzc_pkg::lzc32(32'(input_bits));
            ret   = {ret32.all_zero, COUNT_WIDTH'(ret32.count)};
        end else if (veryl_testcase_Package68G::WIDTH <= 64) begin
            __std_lzc_pkg::__result__64 ret64;
            ret64 = __std_lzc_pkg::lzc64(64'(input_bits));
            ret   = {ret64.all_zero, COUNT_WIDTH'(ret64.count)};
        end else begin
            ret = '1;
            for (int i = 0; i < N_SPLITS; i++) begin
                __std_lzc_pkg::__result__64 ret64;

                ret64 = __std_lzc_pkg::lzc64(64'((input_bits >> (64 * i))));
                if (!ret64.all_zero) begin
                    ret = (RESULT_WIDTH'((i << 6))) | (RESULT_WIDTH'(ret64.count));
                    break;
                end
            end
        end

        return ret;
    endfunction
endmodule
//# sourceMappingURL=../map/68_std_2.sv.map
