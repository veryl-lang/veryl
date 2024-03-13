module veryl_testcase_Module39 (
    veryl_testcase_Interface39.master mst,
    veryl_testcase_Interface39.slave slv
);
    always_comb mst.a = slv.a;
endmodule

interface veryl_testcase_Interface39;
    logic a;

    modport master (
        output a
    );

    modport slave (
        input a
    );
endinterface
