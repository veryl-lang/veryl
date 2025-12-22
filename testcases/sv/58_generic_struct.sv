package veryl_testcase_Package58;
    typedef int unsigned     B;
    typedef longint unsigned C;
endpackage

module veryl_testcase_Module58;
    typedef struct packed {
        veryl_testcase_Package58::B A;
    } __StructA__Package58_B;
    typedef struct packed {
        veryl_testcase_Package58::C A;
    } __StructA__Package58_C;
    typedef struct packed {
        C                           A;
    } __StructA__C;

    typedef int signed C;

    typedef struct packed {
        veryl_testcase_Package58::C B;
    } __StructB__Package58_C;
    typedef struct packed {
        C                           B;
    } __StructB__C;

    typedef struct packed {
        C B;
        C C;
    } __StructC__C__C;

    __StructA__Package58_B _a; always_comb _a = 0;
    __StructA__Package58_C _b; always_comb _b = 0;
    __StructA__C           _c; always_comb _c = 0;
    __StructB__Package58_C _d; always_comb _d = 0;
    __StructB__Package58_C _f; always_comb _f = 0;
    __StructB__C           _e; always_comb _e = 0;
    __StructC__C__C        _g; always_comb _g = 0;
endmodule
//# sourceMappingURL=../map/58_generic_struct.sv.map
