module veryl_testcase_Module97A (
    input var logic i_rst_a  ,
    input var logic i_rst_b  ,
    input var logic i_rst_c_n
);
endmodule

module veryl_testcase_Module97B (
    input  var logic         i_rst,
    output var logic         o_rst,
    output var logic [2-1:0] o_a  
);
    typedef struct packed {
        logic rst;
    } StructA;

    logic   a_n; always_comb a_n = 1'b0;
    StructA b  ;

    always_comb b.rst = 1'b1;

    always_comb begin
        o_rst = 1'b1;
    end

    always_comb o_a[0] = i_rst == 1'b1;
    always_comb o_a[1] = b.rst != 1'b1;

    veryl_testcase_Module97A u (
        .i_rst_a   (1'b1),
        .i_rst_b   (1'b1),
        .i_rst_c_n (a_n )
    );
endmodule
//# sourceMappingURL=../map/97_reset_value_literal.sv.map
