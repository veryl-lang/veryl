module trailing_comments // the module
(
    input  logic a, // first port
    output logic b  // second port
);
    logic c; // a variable
    assign c = a; // copy
    assign b = c;
endmodule
