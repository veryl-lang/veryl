module veryl_testcase_Module35;
    logic aa;
    always_comb aa = 1;

    veryl_testcase_Module35B xx (
        .aa   (aa),
        .bb   (  ),
        .bbbb (  )
    );
endmodule

module veryl_testcase_Module35B (
    input  logic [32-1:0] aa  ,
    output logic [32-1:0] bb  ,
    output logic [32-1:0] bbbb
);
    always_comb begin
        bb   = 0;
        bbbb = 0;
    end
endmodule
//# sourceMappingURL=../map/testcases/sv/35_unconnected_port.sv.map
