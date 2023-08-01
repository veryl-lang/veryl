module veryl_testcase_Module38 (
    veryl_testcase_Interface38.master mst,
    veryl_testcase_Interface38.slave slv
);
    assign mst.a = slv.a;
endmodule

interface veryl_testcase_Interface38;
    logic a;

    modport master (
        output a
    );

    modport slave (
        input a
    );
endinterface
