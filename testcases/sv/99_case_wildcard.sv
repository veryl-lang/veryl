module veryl_testcase_Module99 (
    input  var logic [8-1:0] i_a,
    output var logic [8-1:0] o_b,
    output var logic [8-1:0] o_c
);
    // Statement case with wildcard 'x' and range
    always_comb begin
        case (i_a) inside
            8'b0000_00xx   : o_b = 8'd1;
            8'b0001_xxxx   : o_b = 8'd2;
            [8'd100:8'd200]: o_b = 8'd3;
            default        : o_b = 8'd0;
        endcase
    end

    // Expression case with wildcard 'x' and range
    always_comb o_c = (((i_a) ==? (8'b0000_00xx)) ? (
        8'd1
    ) : ((i_a) ==? (8'b0001_xxxx)) ? (
        8'd2
    ) : ((i_a) inside {[8'd100:8'd200]}) ? (
        8'd3
    ) : (
        8'd0
    ));
endmodule
//# sourceMappingURL=../map/99_case_wildcard.sv.map
