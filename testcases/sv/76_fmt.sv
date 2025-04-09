module veryl_testcase_Module76;

    veryl_testcase_Module76A #( .A (1), .B (2) ) u0 ( .x  (1), .y  ( ) );

    veryl_testcase_Module76A #( .A (1), .B (2) ) u1 ( .x  (1), .y  ( ) );
    veryl_testcase_Module76A #( .A (1), .B (2) ) u2 ( .x  (1), .y  ( ) );
    veryl_testcase_Module76A #( .A (1), .B (2) ) u3 ( .x  (1), .y  ( ) );
    veryl_testcase_Module76A #( .A (1), .B (2) ) u4 ( .x  (1), .y  ( ) );

    logic a ; always_comb a  = 1;
    logic _b; always_comb _b = ((a == 1) ? (
        128'h11111111111111111111
    ) : (a == 2) ? (
        128'h22222222222222222222
    ) : (a == 3) ? (
        128'h33333333333333333333
    ) : (
        128'h44444444444444444444
    ));

    logic _c; always_comb _c = ((a == 1) ? ( 128'h11111111111111111111 ) : (a == 2) ? ( 128'h22222222222222222222 ) : (a == 3) ? ( 128'h33333333333333333333 ) : ( 128'h44444444444444444444 ));
endmodule

module veryl_testcase_Module76A #(
    parameter int unsigned A = 1,
    parameter int unsigned B = 1
) (
    input  var logic x,
    output var logic y
);
    always_comb y = x;
endmodule
//# sourceMappingURL=../map/76_fmt.sv.map
