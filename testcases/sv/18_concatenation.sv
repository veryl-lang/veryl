module Module18 ;
    assign a = {a[10:0], b};
    assign a = {{10{a[10:0]}}, {4{b}}};
endmodule
