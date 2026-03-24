module veryl_testcase___Module87A__3__logic_3 (
    output var logic [3-1:0] a,
    output var logic [3-1:0] b
);
    always_comb begin
        a = '0;
        b = '0;
    end
endmodule

module veryl_testcase___Module87B__1__2__3;



    logic [3-1:0] a;
    logic [3-1:0] b;

    veryl_testcase___Module87A__3__logic_3 u (
        .a (a),
        .b (b)
    );
endmodule

module veryl_testcase_Module87C;
    veryl_testcase___Module87B__1__2__3 u ();
endmodule
//# sourceMappingURL=../map/87_generic_const.sv.map
