interface veryl_testcase_Interface70;
    logic en;

    modport port (
        output en
    );
endinterface

module veryl_testcase_Module70 (
    interface b
);
    veryl_testcase_Interface70 a ();

    veryl_testcase_Module70A u (
        .a (a),
        .b (b)
    );
endmodule

module veryl_testcase_Module70A (
    veryl_testcase_Interface70.port a,
    interface b
);
endmodule
//# sourceMappingURL=../map/testcases/sv/70_interface_connect.sv.map
