module veryl_testcase_Module68L
    import veryl_testcase_Package68K::*;
(
    input  var logic [N-1:0]        i_sel,
    input  var logic [N-1:0][8-1:0] i_d  ,
    output var logic [8-1:0]        o_d  
);



    always_comb o_d = __std___select_onehot__Package68K_N__logic_8(i_sel, i_d);

    function automatic logic [8-1:0] __std___select_onehot__Package68K_N__logic_8(
        input var logic [veryl_testcase_Package68K::N-1:0]        sel ,
        input var logic [veryl_testcase_Package68K::N-1:0][8-1:0] data
    ) ;
        localparam int unsigned DEPTH = $clog2(veryl_testcase_Package68K::N);
        int unsigned                                           next_n;
        logic        [veryl_testcase_Package68K::N-1:0][8-1:0] next_d;

        next_n = veryl_testcase_Package68K::N;
        for (int i = 0; i < veryl_testcase_Package68K::N; i++) begin
            if (sel[i]) begin
                next_d[i] = data[i];
            end else begin
                next_d[i] = logic [8-1:0]'(0);
            end
        end

        for (int _i = 0; _i < DEPTH; _i++) begin
            int unsigned current_n;
            logic        [veryl_testcase_Package68K::N-1:0][8-1:0]                               current_d;

            current_n = next_n;
            current_d = next_d;

            next_n = (current_n / 2) + (current_n % 2);
            for (int j = 0; j < next_n; j++) begin
                if ((j + 1) == next_n && (current_n % 2) == 1) begin
                    next_d[j] = current_d[2 * j + 0];
                end else begin
                    next_d[j] = logic [8-1:0]'((current_d[2 * j + 0] | current_d[2 * j + 1]));
                end
            end
        end

        return next_d[0];
    endfunction
endmodule
//# sourceMappingURL=../map/68_std_6.sv.map
