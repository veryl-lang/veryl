/// Generic package test for doc comment
package veryl_testcase___Package57A__1;
    localparam int unsigned X = 1;
endpackage

/// Generic package test for doc comment
package veryl_testcase___Package57A__2;
    localparam int unsigned X = 2;
endpackage

/// Generic package test for doc comment
package veryl_testcase___Package57B__3;
    localparam int unsigned X = 3;
endpackage
package veryl_testcase___Package57B__4;
    localparam int unsigned X = 4;
endpackage
package veryl_testcase___Package57B__Package57D_Y;
    localparam int unsigned X = veryl_testcase_Package57D::Y;
endpackage

package veryl_testcase___Package57C__2;
    typedef struct packed {
        logic [2-1:0] c;
    } StructC;
endpackage

package veryl_testcase_Package57D;
    localparam int unsigned Y = 1;
endpackage

module veryl_testcase_Module57;
    import veryl_testcase_Package57D::Y;
    localparam int unsigned     A = veryl_testcase___Package57A__1::X;
    localparam longint unsigned B = veryl_testcase___Package57A__2::X;
    localparam int unsigned     C = veryl_testcase___Package57B__3::X;
    localparam int unsigned     E = veryl_testcase___Package57B__3::X;
    localparam longint unsigned D = veryl_testcase___Package57B__4::X;
    localparam longint unsigned F = veryl_testcase___Package57B__Package57D_Y::X;

    veryl_testcase___Package57C__2::StructC _e  ;
    always_comb _e.c = 1;
endmodule
//# sourceMappingURL=../map/testcases/sv/57_generic_package.sv.map
