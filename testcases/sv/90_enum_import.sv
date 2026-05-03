package veryl_testcase_Package90;
    import veryl_testcase_Package90::EnumPkg_P;

    typedef enum logic [3-1:0] {
        EnumPkg_P,
        EnumPkg_Q,
        EnumPkg_R
    } EnumPkg;
endpackage



module veryl_testcase_Module90A;
    import veryl_testcase_Package90::EnumPkg_P;

    import veryl_testcase_Package90::EnumPkg_P;



endmodule

module veryl_testcase_Module90B
    import veryl_testcase_Package90::EnumPkg_P;

    import veryl_testcase_Package90::EnumPkg_P;

#(
    parameter veryl_testcase_Package90::EnumPkg V = veryl_testcase_Package90::EnumPkg_P
);


    veryl_testcase_Package90::EnumPkg _v; always_comb _v = V;
endmodule

module veryl_testcase_Module90C
    import veryl_testcase_Package90::EnumPkg_P;

    import veryl_testcase_Package90::EnumPkg_P;

(
    output var logic o_d
);



    veryl_testcase_Package90::EnumPkg a  ;
    always_comb a   = veryl_testcase_Package90::EnumPkg_P;
    always_comb o_d = ((a) inside {veryl_testcase_Package90::EnumPkg_P, veryl_testcase_Package90::EnumPkg_Q, veryl_testcase_Package90::EnumPkg_R});
endmodule

module veryl_testcase_Module90D;
    import veryl_testcase_Package90::EnumPkg_P;

    if (1) begin :g0
        import veryl_testcase_Package90::EnumPkg_P;
        veryl_testcase_Package90::EnumPkg _a; always_comb _a = veryl_testcase_Package90::EnumPkg_P;
    end

    if (1'b1) begin :g1
        import veryl_testcase_Package90::EnumPkg_P;
        veryl_testcase_Package90::EnumPkg _a; always_comb _a = veryl_testcase_Package90::EnumPkg_P;
    end else begin :g1
        import veryl_testcase_Package90::EnumPkg_P;
        veryl_testcase_Package90::EnumPkg _a; always_comb _a = veryl_testcase_Package90::EnumPkg_P;
    end

    for (genvar i = 0; i < 1; i++) begin :g2
        import veryl_testcase_Package90::EnumPkg_P;
        veryl_testcase_Package90::EnumPkg _a; always_comb _a = veryl_testcase_Package90::EnumPkg_P;
    end
endmodule

module veryl_testcase_Module90E;
    import veryl_testcase_Package90::EnumPkg_P;



    typedef enum logic [3-1:0] {
        EnumLocal_A,
        EnumLocal_B,
        EnumLocal_C,
        EnumLocal_D,
        EnumLocal_E,
        EnumLocal_F
    } EnumLocal;



    EnumLocal a;
    logic     x;

    always_comb a = EnumLocal_A;
    always_comb x = ((a) inside {EnumLocal_A, EnumLocal_B, EnumLocal_C, EnumLocal_D, EnumLocal_E, EnumLocal_F});
endmodule

interface veryl_testcase_Interface90A;
    import veryl_testcase_Package90::EnumPkg_P;

    import veryl_testcase_Package90::EnumPkg_P;



endinterface

interface veryl_testcase_Interface90B
    import veryl_testcase_Package90::EnumPkg_P;

    import veryl_testcase_Package90::EnumPkg_P;

#(
    parameter veryl_testcase_Package90::EnumPkg V = veryl_testcase_Package90::EnumPkg_P
);


    veryl_testcase_Package90::EnumPkg _v; always_comb _v = V;
endinterface

package veryl_testcase_Package90Z;
    import veryl_testcase_Package90::EnumPkg_P;

    import veryl_testcase_Package90::EnumPkg_P;



    localparam veryl_testcase_Package90::EnumPkg _C = veryl_testcase_Package90::EnumPkg_P;
endpackage
//# sourceMappingURL=../map/90_enum_import.sv.map
