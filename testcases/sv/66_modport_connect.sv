interface veryl_testcase_Interface66;
    logic en;

    modport port (
        output en
    );
endinterface

module veryl_testcase_Module66 (
    veryl_testcase_Interface66.port a,
    interface b,
    interface.port c
);

    veryl_testcase_Module66A u (
        .a (a),
        .b (b),
        .c (c)
    );
endmodule

module veryl_testcase_Module66A (
    veryl_testcase_Interface66.port a,
    interface b,
    interface.port c
);
endmodule
//# sourceMappingURL=../map/testcases/sv/66_modport_connect.sv.map
