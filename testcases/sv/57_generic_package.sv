/// Generic package test for doc comment
package veryl_testcase___Package57A__1;
    localparam int unsigned X = 1;
endpackage

/// Generic package test for doc comment
package veryl_testcase___Package57A__2;
    localparam int unsigned X = 2;
endpackage

/// Generic package test for doc comment
package veryl_testcase___Package57B__5;
    localparam int unsigned X = 5;
endpackage
package veryl_testcase___Package57B__3;
    localparam int unsigned X = 3;
endpackage
package veryl_testcase___Package57B__4;
    localparam int unsigned X = 4;
endpackage
package veryl_testcase___Package57B__Package57E_Y;
    localparam int unsigned X = veryl_testcase_Package57E::Y;
endpackage

package veryl_testcase___Package57C__2;
    typedef struct packed {
        logic [2-1:0] c;
    } StructC;
endpackage

package veryl_testcase___Package57D__1;
    typedef struct packed {
        logic [1-1:0] d0;
        logic [2-1:0] d1;
    } __StructD__2;
endpackage

package veryl_testcase_Package57E;
    localparam int unsigned Y = 1;
endpackage


module veryl_testcase_Module57F;
endmodule


package veryl_testcase___Package57F__Module57F;


endpackage

package veryl_testcase___Package57G__i32;
    typedef int signed TYPE;
endpackage
package veryl_testcase___Package57G__u32;
    typedef int unsigned TYPE;
endpackage
package veryl_testcase___Package57G__bool;
    typedef logic TYPE;
endpackage

module veryl_testcase_Module57;
    import veryl_testcase_Package57E::Y;
    import veryl_testcase___Package57B__5::*;


    localparam int unsigned     A = veryl_testcase___Package57A__1::X;
    localparam longint unsigned B = veryl_testcase___Package57A__2::X;
    localparam int unsigned     C = veryl_testcase___Package57B__3::X;
    localparam int unsigned     D = veryl_testcase___Package57B__3::X;
    localparam longint unsigned E = veryl_testcase___Package57B__4::X;
    localparam longint unsigned F = veryl_testcase___Package57B__Package57E_Y::X;
    localparam longint unsigned G = X;

    veryl_testcase___Package57C__2::StructC      _c   ;
    veryl_testcase___Package57D__1::__StructD__2 _d   ;
    always_comb _c.c  = 1;
    always_comb _d.d0 = 0;
    always_comb _d.d1 = 1;

    veryl_testcase_Module57F u ();

    veryl_testcase___Package57G__i32::TYPE  _e; always_comb _e = 0;
    veryl_testcase___Package57G__u32::TYPE  _f; always_comb _f = 0;
    veryl_testcase___Package57G__bool::TYPE _g; always_comb _g = 1'b0;
endmodule
//# sourceMappingURL=../map/57_generic_package.sv.map
