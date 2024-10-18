module veryl_testcase_Module71 #(
    parameter type param_type = logic
);
    typedef logic [32-1:0] type_type;

    typedef struct packed {
        logic a;
    } struct_type;

    veryl_testcase_Module71A #(.T1 (param_type), .T2 (type_type), .T3 (struct_type), .T4 (logic [10-1:0])) m ();
endmodule

module veryl_testcase_Module71A #(
    parameter type T1 = logic,
    parameter type T2 = logic,
    parameter type T3 = logic,
    parameter type T4 = logic
);
endmodule
//# sourceMappingURL=../map/testcases/sv/71_type_parameter.sv.map
