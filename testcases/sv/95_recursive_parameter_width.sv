module veryl_testcase_Module95 #(
    parameter int unsigned             STAGE = 2    ,
    parameter int unsigned             WIDTH = 2    ,
    parameter logic        [STAGE-1:0] FORKS = 2'b01,
    parameter logic        [WIDTH-1:0] MASK  = 2'b10
) (
    output var logic o_fork
);
    always_comb o_fork = FORKS[0] & MASK[0];

    // A component instantiating itself names its own scope, so a parameter a
    // declared width names must not read as a use before its declaration.
    // STAGE, which sizes FORKS, is bound here; WIDTH, which sizes the equally
    // bound MASK, is not.
    if (STAGE >= 2) begin :deeper
        veryl_testcase_Module95 #(
            .STAGE (STAGE - 1),
            .FORKS (1'b1     ),
            .MASK  (2'b01    )
        ) u (
            .o_fork ()
        );
    end
endmodule
//# sourceMappingURL=../map/95_recursive_parameter_width.sv.map
