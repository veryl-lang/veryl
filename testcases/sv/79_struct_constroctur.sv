module veryl_testcase_Module79;
    typedef struct packed {
        logic [10-1:0] a ;
        logic [10-1:0] bb;
    } StructA;

    StructA _a; always_comb _a = '{
        a : 10000,
        bb: 10   
    };

    StructA _b; always_comb _b = '{
        a : 10,
        default: 0
    };

    StructA _c; always_comb _c = '{a : 10, bb: 10};
endmodule
//# sourceMappingURL=../map/testcases/sv/79_struct_constroctur.sv.map
