module veryl_testcase_Module58;
    typedef struct packed {
        veryl_testcase_Package58::B A;
    } __StructA__Package58_B;
    typedef struct packed {
        veryl_testcase_Package58::C A;
    } __StructA__Package58_C;
    typedef struct packed {
        C A;
    } __StructA__C;

    typedef int signed C;

    typedef struct packed {
        veryl_testcase_Package58::C B;
    } __StructB__Package58_C;
    typedef struct packed {
        C B;
    } __StructB__C;

    __StructA__Package58_B _a;
    __StructA__Package58_C _b;
    __StructA__C           _c;
    __StructB__Package58_C _d;
    __StructB__C _e;
endmodule

package veryl_testcase_Package58;
    typedef int unsigned     B;
    typedef longint unsigned C;
endpackage
//# sourceMappingURL=../map/testcases/sv/58_generic_struct.sv.map
