package veryl_testcase_Package73;
    localparam bit A = 0;
endpackage

module veryl_testcase___Module73A__0 (
    input  logic i_a,
    input  logic i_b,
    input  logic i_c,
    output logic o_d
);
    always_comb o_d = 0;
endmodule
module veryl_testcase___Module73A__1 (
    input  logic i_a,
    input  logic i_b,
    input  logic i_c,
    output logic o_d
);
    always_comb o_d = 0;
endmodule

module veryl_testcase_Module73B;
    veryl_testcase___Module73A__0 u0 (
        .i_a (veryl_testcase_Package73::A),
        .i_b (0                          ),
        .i_c (0),
        .o_d ( )
    )
    ;
    veryl_testcase___Module73A__1 u1 (
        .i_a (veryl_testcase_Package73::A),
        .i_b (1                          ),
        .i_c (0),
        .o_d ( )
    )
    ;
    veryl_testcase___Module73A__1 u2 (
        .i_a (0),
        .i_b (0),
        .i_c (0),
        .o_d ( )

    );
endmodule

module veryl_testcase_Module73C;
    function automatic void __FuncC__0(
        input logic i_a,
        input logic i_b,
        input logic i_c
    ) ;
    endfunction
    function automatic void __FuncC__1(
        input logic i_a,
        input logic i_b,
        input logic i_c
    ) ;
    endfunction

    always_comb begin
        __FuncC__0(veryl_testcase_Package73::A, 0, 1);
        __FuncC__1(veryl_testcase_Package73::A, 1, 1);
        __FuncC__1(0, 0, 1);
    end
endmodule

module veryl_testcase_Module73D;
    function automatic bit   __FuncD__0(
        input logic i_a,
        input logic i_b,
        input logic i_c
    ) ;
        return 0;
    endfunction
    function automatic bit   __FuncD__1(
        input logic i_a,
        input logic i_b,
        input logic i_c
    ) ;
        return 0;
    endfunction

    bit _d;
    bit _e;
    bit _f;

    always_comb begin
        _d = __FuncD__0(veryl_testcase_Package73::A, 0, 1);
        _e = __FuncD__1(veryl_testcase_Package73::A, 1, 1);
        _f = __FuncD__1(0, 0, 1);
    end
endmodule
//# sourceMappingURL=../map/testcases/sv/73_port_default_value.sv.map
