module veryl_testcase_Module39 (
    veryl_testcase_Interface39.master mst,
    veryl_testcase_Interface39.slave  slv
);
    logic a    ;
    always_comb mst.a = a;
    always_comb a     = slv.get_a();

    veryl_testcase_ModuleAnother module_another (
        .mst (mst),
        .slv (slv)
    );
endmodule

interface veryl_testcase_Interface39;
    logic a;

    function automatic logic get_a() ;
        return a;
    endfunction

    modport master (
        output a
    );

    modport slave (
        input  a    ,
        import get_a
    );
endinterface

module veryl_testcase_ModuleAnother (
    veryl_testcase_Interface39.master mst,
    veryl_testcase_Interface39.slave  slv
);
endmodule
//# sourceMappingURL=../map/testcases/sv/39_modport.sv.map
