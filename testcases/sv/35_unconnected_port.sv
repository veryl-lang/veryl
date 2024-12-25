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
    input  int unsigned aa  ,
    output int unsigned bb  ,
    output int unsigned bbbb
);
    always_comb begin
        bb   = 0;
        bbbb = 0;
    end
endmodule
//# sourceMappingURL=../map/testcases/sv/35_unconnected_port.sv.map
