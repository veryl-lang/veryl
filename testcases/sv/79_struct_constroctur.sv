module veryl_testcase_Module79;
    typedef struct packed {
        logic [10-1:0] a ;
        logic [10-1:0] bb;
    } StructA;

    StructA _a; always_comb _a = StructA'{
        a : 10000,
        bb: 10   
    };

    StructA _b; always_comb _b = StructA'{
        a : 10,
        default: 0
    };

    StructA _c; always_comb _c = StructA'{a : 10, bb: 10};

    typedef union packed {
        logic   [20-1:0] dd      ;
        StructA          struct_a;
    } UnionA;

    typedef struct packed {
        UnionA uu;
    } StructWithUnion;

    StructWithUnion _e; always_comb _e = StructWithUnion'{
        uu: StructA'{
            a : 5, bb: 5
        }
    };
endmodule
//# sourceMappingURL=../map/79_struct_constroctur.sv.map
