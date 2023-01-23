module veryl_testcase_Module13;
    logic  a;
    logic  X;

    // bit select
    assign a = X[0];

    // range select
    assign a = X[1:0];

    // position and width
    assign a = X[1+:2];
    assign a = X[1-:2];

    // index by step
    assign a = X[1*(2)+:(2)];
endmodule
