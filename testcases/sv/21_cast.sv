module Module21 ;
    logic  a;
    logic  b;
    typedef 
    enum logic  {
        A,
        B
    } EnumA;
    typedef 
    enum logic  {
        C,
        D
    } EnumB;

    assign a = EnumA'(EnumB'(b));
endmodule
