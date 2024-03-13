module veryl_testcase_Module13;
    logic a;
    logic X;

    // bit select
    always_comb a = X[0];

    // range select
    always_comb a = X[1:0];

    // position and width
    always_comb a = X[1+:2];
    always_comb a = X[1-:2];

    // index by step
    always_comb a = X[1*(2)+:(2)];
endmodule
