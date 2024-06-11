module veryl_testcase_Module35;
    logic aa;
    always_comb aa = 1;

    veryl_testcase_Module35B xx (
        .aa   (aa  ),
        .bb   (),
        .bbbb ()
    );
endmodule

module veryl_testcase_Module35B (
    input int unsigned aa  ,
    input int unsigned bb  ,
    input int unsigned bbbb
);
endmodule
//# sourceMappingURL=../map/testcases/sv/35_unconnected_port.sv.map
