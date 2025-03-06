

package veryl_testcase___Package55__8__16;
    typedef logic [8-1:0]  data_a;
    typedef logic [16-1:0] data_b;
endpackage

module veryl_testcase_Module55;
    veryl_testcase___Module55A__Module55B u0 ();
    veryl_testcase___Module55A__Module55C u1 ();
    veryl_testcase___Module55E__Module55C u2 ();
    veryl_testcase___Module55E__Module55D u3 ();
    veryl_testcase___Module55F__Module55C u4 ();
    veryl_testcase___Module55F__Module55B u5 ();
    veryl_testcase___Module55H__10 u6 ();
    veryl_testcase___Module55H__10 u7 ();
    veryl_testcase___Module55I____Package55__8__16 u8 ();
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
//# sourceMappingURL=../map/testcases/sv/55_generic_module.sv.map
