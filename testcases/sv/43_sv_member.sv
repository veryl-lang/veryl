module veryl_testcase_Module43 (
    InterfaceA.mp port
);
    StructA a;

    logic _b; always_comb _b = a.memberA;

    InterfaceA c ();

    logic _d; always_comb _d = c.memberA;
endmodule
//# sourceMappingURL=../map/43_sv_member.sv.map
