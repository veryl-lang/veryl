module veryl_testcase_Module20;
    logic a;
    logic b;
    logic x;
    logic y;

    always_comb a = ((x) ? (
        1
    ) : (
        ((y) ? (
            1
        ) : (
            2
        ))
    ));

    always_comb b = ((a == 1) ? (
        0
    ) : (a == 2) ? (
        1
    ) : (
        2
    ));
endmodule
