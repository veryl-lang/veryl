module Module20 ;
    logic  a;
    logic  b;
    logic  x;
    logic  y;

    assign a = ((x) ? (
        1
    ) : (
        ((y) ? (
            1
        ) : (
            2
        ))
    ));

    assign b = ((a == 1) ? (
        0
    ) : (a == 2) ? (
        1
    ) : (
        2
    ));
endmodule
