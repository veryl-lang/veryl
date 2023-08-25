module veryl_testcase_Module41;
    typedef enum logic {
        Boolean_True = 1,
        Boolean_False = 0
    } Boolean;

    typedef union packed {
        logic   b;
        Boolean c;
    } A;

    A foo  ;
    assign foo.b = 1'b0;

    A bar  ;
    assign bar.c = Boolean_True;
endmodule
