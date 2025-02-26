module veryl_testcase_Module20;
    logic a;
    logic b;
    logic c;
    logic x; always_comb x = 1;
    logic y; always_comb y = 1;

    always_comb a = ((x) ? ( 1 ) : ( ((y) ? ( 1 ) : ( 2 )) ));

    always_comb b = (((a) ==? (1)) ? (
        0
    ) : ((a) ==? (2)) ? (
        1
    ) : ((a) ==? (3)) ? (
        2
    ) : ((a) ==? (4)) ? (
        2
    ) : ((a) inside {[5:7]}) ? (
        3
    ) : (
        4
    ));

    always_comb c = (((a == 1) == 1'b1) ? (
        0
    ) : ((a == 2) == 1'b1) ? (
        1
    ) : ((a == 3) == 1'b1) ? (
        2
    ) : ((a == 4) == 1'b1) ? (
        2
    ) : (
        3
    ));
endmodule
//# sourceMappingURL=../map/testcases/sv/20_if_case_switch_expression.sv.map
