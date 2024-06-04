module veryl_testcase_Module39 (
    veryl_testcase_Interface39.master mst,
    veryl_testcase_Interface39.slave  slv
);
    logic a    ;
    always_comb mst.a = a;
    always_comb a     = slv.get_a();
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
