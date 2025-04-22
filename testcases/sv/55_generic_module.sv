

package veryl_testcase___Package55__8__16;
    typedef logic [8-1:0]  data_a;
    typedef logic [16-1:0] data_b;
endpackage

package veryl_testcase_Package55K;
    typedef enum logic [1-1:0] {
        FooBar_FOO,
        FooBar_BAR
    } FooBar;
endpackage

module veryl_testcase_Module55;



    veryl_testcase___Module55A__Module55B u_a0 ();
    veryl_testcase___Module55A__Module55C u_a1 ();
    veryl_testcase___Module55E__Module55C u_e0 ();
    veryl_testcase___Module55E__Module55D u_e1 ();
    veryl_testcase___Module55F__Module55C u_f0 ();
    veryl_testcase___Module55F__Module55B u_f1 ();
    veryl_testcase___Module55H__10 u_h0 ();
    veryl_testcase___Module55H__10 u_h1 ();
    veryl_testcase___Module55I____Package55__8__16 u_i0 ();
    veryl_testcase___Module55I____Package55__8__16 u_j0 ();
    veryl_testcase___Module55K__Package55K_FooBar_FOO u_k0 ();
    veryl_testcase___Module55K__Package55K_FooBar_BAR u_k1 ();
endmodule


module veryl_testcase___Module55A__Module55B;
    veryl_testcase_Module55B u ();
endmodule
module veryl_testcase___Module55A__Module55C;
    veryl_testcase_Module55C u ();
endmodule
module veryl_testcase___Module55A__Module55D;
    veryl_testcase_Module55D u ();
endmodule

module veryl_testcase_Module55B;
endmodule

module veryl_testcase_Module55C;
endmodule

module veryl_testcase_Module55D;
endmodule

module veryl_testcase___Module55E__Module55C;
    veryl_testcase___Module55A__Module55C u ();
endmodule
module veryl_testcase___Module55E__Module55D;
    veryl_testcase___Module55A__Module55D u ();
endmodule

module veryl_testcase___Module55F__Module55C;
    veryl_testcase_Module55C u ();
endmodule
module veryl_testcase___Module55F__Module55B;
    veryl_testcase_Module55B u ();
endmodule


module veryl_testcase___Module55H__10;
    typedef struct packed {
        logic [10-1:0] value;
    } __StructH__10;

    __StructH__10 _a; always_comb _a = 0;
endmodule

module veryl_testcase___Module55I____Package55__8__16;
    veryl_testcase___Package55__8__16::data_a _a; always_comb _a = 0;
    veryl_testcase___Package55__8__16::data_b _b; always_comb _b = 0;
endmodule

module veryl_testcase___Module55K__Package55K_FooBar_FOO;
    veryl_testcase_Package55K::FooBar _foobar; always_comb _foobar = veryl_testcase_Package55K::FooBar_FOO;
endmodule
module veryl_testcase___Module55K__Package55K_FooBar_BAR;
    veryl_testcase_Package55K::FooBar _foobar; always_comb _foobar = veryl_testcase_Package55K::FooBar_BAR;
endmodule
//# sourceMappingURL=../map/55_generic_module.sv.map
