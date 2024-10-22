interface veryl_testcase_Interface70A;
    logic en;

    modport port (
        output en
    );
endinterface

interface veryl_testcase___Interface70B__8;
    logic [8-1:0] value;

    modport port (
        output value
    );
endinterface

module veryl_testcase_Module70 (
    interface c
);
    veryl_testcase_Interface70A a ();
    veryl_testcase___Interface70B__8 b ();

    veryl_testcase_Module70A u (
        .a (a),
        .b (b),
        .c (c)
    );
endmodule

module veryl_testcase_Module70A (
    veryl_testcase_Interface70A.port      a,
    veryl_testcase___Interface70B__8.port b,
    interface c
);
endmodule
//# sourceMappingURL=../map/testcases/sv/70_interface_connect.sv.map
