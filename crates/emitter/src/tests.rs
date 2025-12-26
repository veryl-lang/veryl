use crate::Emitter;
use std::path::PathBuf;
use veryl_analyzer::{Analyzer, attribute_table, symbol_table};
use veryl_metadata::{ClockType, Metadata, ResetType};
use veryl_parser::Parser;

#[track_caller]
fn emit(metadata: &Metadata, code: &str) -> String {
    symbol_table::clear();
    attribute_table::clear();

    let parser = Parser::parse(&code, &"").unwrap();
    let analyzer = Analyzer::new(metadata);

    analyzer.analyze_pass1(&"prj", &"", &parser.veryl);
    Analyzer::analyze_post_pass1();
    analyzer.analyze_pass2(&"prj", &"", &parser.veryl);

    let mut emitter = Emitter::new(
        metadata,
        &PathBuf::from("test.veryl"),
        &PathBuf::from("test.sv"),
        &PathBuf::from("test.sv.map"),
    );
    emitter.emit(&"prj", &parser.veryl);
    emitter.as_str().to_string()
}

#[test]
fn prefix_suffix_clock_posedge_reset_high() {
    let code = r#"module ModuleA (
    clk: input clock,
    rst: input reset,
) {
    inst u: ModuleB (
        clk,
        rst,
    );

    let _a: logic = clk;
    let _b: logic = rst;

    var _c: logic;
    always_ff {
        if_reset {
            _c = 0;
        } else {
            _c = 1;
        }
    }
}

module ModuleB (
    clk: input clock,
    rst: input reset,
) {}
"#;

    let expect = r#"module prj_ModuleA (
    input var logic clk_pos_clk_clk_pos  ,
    input var logic rst_high_rst_rst_high
);
    prj_ModuleB u (
        .clk_pos_clk_clk_pos   (clk_pos_clk_clk_pos  ),
        .rst_high_rst_rst_high (rst_high_rst_rst_high)
    );

    logic _a; always_comb _a = clk_pos_clk_clk_pos;
    logic _b; always_comb _b = rst_high_rst_rst_high;

    logic _c;
    always_ff @ (posedge clk_pos_clk_clk_pos, posedge rst_high_rst_rst_high) begin
        if (rst_high_rst_rst_high) begin
            _c <= 0;
        end else begin
            _c <= 1;
        end
    end
endmodule

module prj_ModuleB (
    input var logic clk_pos_clk_clk_pos  ,
    input var logic rst_high_rst_rst_high
);
endmodule
//# sourceMappingURL=test.sv.map
"#;

    let mut metadata = Metadata::create_default("prj").unwrap();

    metadata.build.clock_type = ClockType::PosEdge;
    metadata.build.reset_type = ResetType::AsyncHigh;
    metadata.build.clock_posedge_prefix = Some("clk_pos_".to_string());
    metadata.build.clock_posedge_suffix = Some("_clk_pos".to_string());
    metadata.build.reset_high_prefix = Some("rst_high_".to_string());
    metadata.build.reset_high_suffix = Some("_rst_high".to_string());

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    assert_eq!(ret, expect);
}

#[test]
fn prefix_suffix_clock_negedge_reset_low() {
    let code = r#"module ModuleA (
    clk: input clock,
    rst: input reset,
) {
    inst u: ModuleB (
        clk,
        rst,
    );

    let _a: logic = clk;
    let _b: logic = rst;

    var _c: logic;
    always_ff {
        if_reset {
            _c = 0;
        } else {
            _c = 1;
        }
    }
}

module ModuleB (
    clk: input clock,
    rst: input reset,
) {}
"#;

    let expect = r#"module prj_ModuleA (
    input var logic clk_neg_clk_clk_neg,
    input var logic rst_low_rst_rst_low
);
    prj_ModuleB u (
        .clk_neg_clk_clk_neg (clk_neg_clk_clk_neg),
        .rst_low_rst_rst_low (rst_low_rst_rst_low)
    );

    logic _a; always_comb _a = clk_neg_clk_clk_neg;
    logic _b; always_comb _b = rst_low_rst_rst_low;

    logic _c;
    always_ff @ (negedge clk_neg_clk_clk_neg) begin
        if (!rst_low_rst_rst_low) begin
            _c <= 0;
        end else begin
            _c <= 1;
        end
    end
endmodule

module prj_ModuleB (
    input var logic clk_neg_clk_clk_neg,
    input var logic rst_low_rst_rst_low
);
endmodule
//# sourceMappingURL=test.sv.map
"#;

    let mut metadata = Metadata::create_default("prj").unwrap();

    metadata.build.clock_type = ClockType::NegEdge;
    metadata.build.reset_type = ResetType::SyncLow;
    metadata.build.clock_negedge_prefix = Some("clk_neg_".to_string());
    metadata.build.clock_negedge_suffix = Some("_clk_neg".to_string());
    metadata.build.reset_low_prefix = Some("rst_low_".to_string());
    metadata.build.reset_low_suffix = Some("_rst_low".to_string());

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    assert_eq!(ret, expect);
}

#[test]
fn clock_reset_port_name() {
    let code = r#"module ModuleA (
    i_clk: input clock,
    i_rst: input reset,
) {}
module ModuleB (
    i_clock: input clock,
    i_reset: input reset,
) {
    inst u: ModuleA (
        i_clk: i_clock,
        i_rst: i_reset,
    );
}
"#;

    let expect = r#"module prj_ModuleA (
    input var logic i_clk_p,
    input var logic i_rst_x
);
endmodule
module prj_ModuleB (
    input var logic i_clock_p,
    input var logic i_reset_x
);
    prj_ModuleA u (
        .i_clk_p (i_clock_p),
        .i_rst_x (i_reset_x)
    );
endmodule
//# sourceMappingURL=test.sv.map
"#;

    let mut metadata = Metadata::create_default("prj").unwrap();
    metadata.build.clock_posedge_suffix = Some("_p".to_string());
    metadata.build.reset_low_suffix = Some("_x".to_string());

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    println!("ret\n{}exp\n{}", ret, expect);
    assert_eq!(ret, expect);
}

#[test]
fn hierarchical_referenced_clock_reset_signals() {
    let code = r#"interface FooIF {
    var clk: clock;
    var rst: reset;
}
module ModuleA (
    i_clk: input clock,
    i_rst: input reset,
) {
    inst foo_if: FooIF;
    always_comb {
        foo_if.clk = i_clk;
        foo_if.rst = i_rst;
    }
}
"#;

    let expect = r#"interface prj_FooIF;
    logic clk_p;
    logic rst_x;
endinterface
module prj_ModuleA (
    input var logic i_clk_p,
    input var logic i_rst_x
);
    prj_FooIF foo_if ();
    always_comb begin
        foo_if.clk_p = i_clk_p;
        foo_if.rst_x = i_rst_x;
    end
endmodule
//# sourceMappingURL=test.sv.map
"#;

    let mut metadata = Metadata::create_default("prj").unwrap();
    metadata.build.clock_posedge_suffix = Some("_p".to_string());
    metadata.build.reset_low_suffix = Some("_x".to_string());

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    println!("ret\n{}exp\n{}", ret, expect);
    assert_eq!(ret, expect);
}

#[test]
fn omit_prject_prefix() {
    let code = r#"module ModuleA {
    inst u: InterfaceB::<10>;
}

package PackageA {
}

interface InterfaceA {
}

interface InterfaceB::<N: u32> {
}
"#;

    let expect = r#"module ModuleA;
    __InterfaceB__10 u ();
endmodule

package PackageA;
endpackage

interface InterfaceA;
endinterface

interface __InterfaceB__10;
endinterface
//# sourceMappingURL=test.sv.map
"#;

    let mut metadata = Metadata::create_default("prj").unwrap();

    metadata.build.omit_project_prefix = true;

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    assert_eq!(ret, expect);
}

#[test]
fn expand_case_statement() {
    let code = r#"module ModuleA {
    const y: bit = 1;

    var a: logic;
    let x: logic = 1;

    always_comb {
        case x {
            0: a = 1;
            1: a = 1;
            2: {
                a = 1;
                a = 1;
                a = 1;
            }
            3, 4   : a = 1;
            5..=7  : a = 1;
            y - 1  : a = 1;
            default: a = 1;
        }
    }
}
"#;

    let expect = r#"module ModuleA;
    localparam bit y = 1;

    logic a;
    logic x; always_comb x = 1;

    always_comb begin
        case (1'b1)
            (x) ==? (0): a = 1;
            (x) ==? (1): a = 1;
            (x) ==? (2): begin
                a = 1;
                a = 1;
                a = 1;
            end
            (x) ==? (3), (x) ==? (4    ): a = 1;
            ((x) >= (5)) && ((x) <= (7)): a = 1;
            (x) ==? (y - 1             ): a = 1;
            default                     : a = 1;
        endcase
    end
endmodule
//# sourceMappingURL=test.sv.map
"#;

    let mut metadata = Metadata::create_default("prj").unwrap();

    metadata.build.omit_project_prefix = true;
    metadata.build.expand_inside_operation = true;

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    assert_eq!(ret, expect);
}

#[test]
fn expand_inside_operator() {
    let code = r#"module ModuleA {
    var a: logic;
    var b: logic;

    assign a = inside 1 + 2 / 3 {0, 0..10, 1..=10};
    assign b = outside 1 * 2 - 1 {0, 0..10, 1..=10};
    }
"#;

    let expect = r#"module ModuleA;
    logic a;
    logic b;

    always_comb a = ((1 + 2 / 3) ==? (0) || ((1 + 2 / 3) >= (0)) && ((1 + 2 / 3) < (10)) || ((1 + 2 / 3) >= (1)) && ((1 + 2 / 3) <= (10)));
    always_comb b = !((1 * 2 - 1) ==? (0) || ((1 * 2 - 1) >= (0)) && ((1 * 2 - 1) < (10)) || ((1 * 2 - 1) >= (1)) && ((1 * 2 - 1) <= (10)));
endmodule
//# sourceMappingURL=test.sv.map
"#;

    let mut metadata = Metadata::create_default("prj").unwrap();

    metadata.build.omit_project_prefix = true;
    metadata.build.expand_inside_operation = true;

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    assert_eq!(ret, expect);
}

#[test]
fn expand_case_expression() {
    let code = r#"module ModuleA {
    let a: logic = 1;
    var b: logic;

    assign b = case a {
        1      : 0,
        2      : 1,
        3, 4   : 2,
        5..=7  : 3,
        default: 4,
    };
}
"#;

    let expect = r#"module ModuleA;
    logic a; always_comb a = 1;
    logic b;

    always_comb b = (((a) ==? (1)) ? (
        0
    ) : ((a) ==? (2)) ? (
        1
    ) : ((a) ==? (3)) ? (
        2
    ) : ((a) ==? (4)) ? (
        2
    ) : (((a) >= (5)) && ((a) <= (7))) ? (
        3
    ) : (
        4
    ));
endmodule
//# sourceMappingURL=test.sv.map
"#;

    let mut metadata = Metadata::create_default("prj").unwrap();

    metadata.build.omit_project_prefix = true;
    metadata.build.expand_inside_operation = true;

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    assert_eq!(ret, expect);
}

#[test]
fn async_reset_cast() {
    let code = r#"module ModuleA {
    var a: reset;
    var b: reset_async_high;
    var c: reset_async_low;

    let d: reset_async_high = a as reset_async_high;
    let e: reset_async_low  = a as reset_async_low ;
    let f: reset            = b as reset           ;
    let g: reset            = c as reset           ;
}
"#;

    let expect = r#"module prj_ModuleA;
    logic a;
    logic b;
    logic c;

    logic d; always_comb d = a;
    logic e; always_comb e = ~a;
    logic f; always_comb f = b;
    logic g; always_comb g = ~c;
endmodule
//# sourceMappingURL=test.sv.map
"#;

    let mut metadata = Metadata::create_default("prj").unwrap();

    metadata.build.reset_type = ResetType::AsyncHigh;

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    assert_eq!(ret, expect);
}

#[test]
fn sync_reset_cast() {
    let code = r#"module ModuleA {
    var a: reset;
    var b: reset_sync_high;
    var c: reset_sync_low;

    let d: reset_sync_high = a as reset_sync_high;
    let e: reset_sync_low  = a as reset_sync_low ;
    let f: reset           = b as reset          ;
    let g: reset           = c as reset          ;
}
"#;

    let expect = r#"module prj_ModuleA;
    logic a;
    logic b;
    logic c;

    logic d; always_comb d = ~a;
    logic e; always_comb e = a;
    logic f; always_comb f = ~b;
    logic g; always_comb g = c;
endmodule
//# sourceMappingURL=test.sv.map
"#;

    let mut metadata = Metadata::create_default("prj").unwrap();

    metadata.build.reset_type = ResetType::SyncLow;

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    assert_eq!(ret, expect);
}

#[test]
fn emit_cond_type() {
    let code = r#"module ModuleA (
    i_clk: input clock,
    i_rst: input reset,
) {
    let x: logic = 1;
    var a: logic;
    var b: logic;
    var c: logic;
    var d: logic;
    var e: logic;
    var f: logic;
    var g: logic;
    var h: logic;
    var i: logic;

    always_comb {
        #[cond_type(unique)]
        case x {
            0: a = 1;
            1: a = 1;
        }
        #[cond_type(unique0)]
        case x {
            0: b = 1;
            1: b = 1;
        }
        #[cond_type(priority)]
        case x {
            0: c = 1;
            1: c = 1;
        }
    }

    always_comb {
        #[cond_type(unique)]
        if x == 0 {
            d = 1;
        } else if x == 1 {
            d = 1;
        }
        #[cond_type(unique0)]
        if x == 0 {
            e = 1;
        } else if x == 1 {
            e = 1;
        }
        #[cond_type(priority)]
        if x == 0 {
            f = 1;
        } else if x == 1 {
            f = 1;
        }
    }

    always_ff {
        #[cond_type(unique)]
        if_reset {
            g = 1;
        } else if x == 1 {
            g = 1;
        }
    }
    always_ff {
        #[cond_type(unique0)]
        if_reset {
            h = 1;
        } else if x == 1 {
            h = 1;
        }
    }
    always_ff {
        #[cond_type(priority)]
        if_reset {
            i = 1;
        } else if x == 1 {
            i = 1;
        }
    }
}
"#;

    let expect = r#"module prj_ModuleA (
    input var logic i_clk,
    input var logic i_rst
);
    logic x; always_comb x = 1;
    logic a;
    logic b;
    logic c;
    logic d;
    logic e;
    logic f;
    logic g;
    logic h;
    logic i;

    always_comb begin

        unique case (x) inside
            0: a = 1;
            1: a = 1;
        endcase

        unique0 case (x) inside
            0: b = 1;
            1: b = 1;
        endcase

        priority case (x) inside
            0: c = 1;
            1: c = 1;
        endcase
    end

    always_comb begin

        unique if (x == 0) begin
            d = 1;
        end else if (x == 1) begin
            d = 1;
        end

        unique0 if (x == 0) begin
            e = 1;
        end else if (x == 1) begin
            e = 1;
        end

        priority if (x == 0) begin
            f = 1;
        end else if (x == 1) begin
            f = 1;
        end
    end

    always_ff @ (posedge i_clk, negedge i_rst) begin

        unique if (!i_rst) begin
            g <= 1;
        end else if (x == 1) begin
            g <= 1;
        end
    end
    always_ff @ (posedge i_clk, negedge i_rst) begin

        unique0 if (!i_rst) begin
            h <= 1;
        end else if (x == 1) begin
            h <= 1;
        end
    end
    always_ff @ (posedge i_clk, negedge i_rst) begin

        priority if (!i_rst) begin
            i <= 1;
        end else if (x == 1) begin
            i <= 1;
        end
    end
endmodule
//# sourceMappingURL=test.sv.map
"#;

    let mut metadata = Metadata::create_default("prj").unwrap();

    metadata.build.emit_cond_type = true;

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    assert_eq!(ret, expect);
}

#[test]
fn emit_nested_generic_instances() {
    let code = r#"proto package ProtoPkgA {
    const WIDTH: u32;
    enum Foo: logic<WIDTH> {
        FOO,
    }
}
package PkgA::<W: u32> for ProtoPkgA {
    const WIDTH: u32 = W;
    enum Foo: logic<WIDTH> {
        FOO,
    }
}
package PkgB::<PKG: ProtoPkgA> {
    import PKG::*;
    function Func::<B: u32> -> u32 {
        return Foo::FOO + B;
    }
    struct Struct::<W: u32> {
        foo: Foo     ,
        bar: logic<W>,
    }
}
module Module {
    let _a: i32 = PkgB::<PkgA::<1>>::Func::<1>();
    let _b: i32 = PkgB::<PkgA::<1>>::Func::<2>();
    let _c: i32 = PkgB::<PkgA::<2>>::Func::<1>();
    var _d: PkgB::<PkgA::<1>>::Struct::<2>;
    var _e: PkgB::<PkgA::<2>>::Struct::<1>;
    var _f: PkgB::<PkgA::<2>>::Struct::<2>;
}
"#;

    let expect = r#"

package prj___PkgA__1;
    localparam int unsigned WIDTH = 1;
    typedef enum logic [WIDTH-1:0] {
        Foo_FOO
    } Foo;
endpackage
package prj___PkgA__2;
    localparam int unsigned WIDTH = 2;
    typedef enum logic [WIDTH-1:0] {
        Foo_FOO
    } Foo;
endpackage
package prj___PkgB____PkgA__1;
    import prj___PkgA__1::*;

    function automatic int unsigned __Func__1;
        return prj___PkgA__1::Foo_FOO + 1;
    endfunction
    function automatic int unsigned __Func__2;
        return prj___PkgA__1::Foo_FOO + 2;
    endfunction
    typedef struct packed {
        prj___PkgA__1::Foo         foo;
        logic              [2-1:0] bar;
    } __Struct__2;
endpackage
package prj___PkgB____PkgA__2;
    import prj___PkgA__2::*;

    function automatic int unsigned __Func__1;
        return prj___PkgA__2::Foo_FOO + 1;
    endfunction
    typedef struct packed {
        prj___PkgA__2::Foo         foo;
        logic              [1-1:0] bar;
    } __Struct__1;
    typedef struct packed {
        prj___PkgA__2::Foo         foo;
        logic              [2-1:0] bar;
    } __Struct__2;
endpackage
module prj_Module;
    int signed                         _a; always_comb _a = prj___PkgB____PkgA__1::__Func__1();
    int signed                         _b; always_comb _b = prj___PkgB____PkgA__1::__Func__2();
    int signed                         _c; always_comb _c = prj___PkgB____PkgA__2::__Func__1();
    prj___PkgB____PkgA__1::__Struct__2 _d;
    prj___PkgB____PkgA__2::__Struct__1 _e;
    prj___PkgB____PkgA__2::__Struct__2 _f;
endmodule
//# sourceMappingURL=test.sv.map
"#;

    let metadata = Metadata::create_default("prj").unwrap();

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    println!("ret\n{}\nexp\n{}", ret, expect);
    assert_eq!(ret, expect);
}

#[test]
fn emit_connect_operation() {
    let code = r#"interface InterfaceA {
    var a: logic;
    var b: logic;
    var c: logic;
    var d: logic;
    var e: tri logic;
    var f: tri logic;
    modport mp_0 {
        a: input ,
        b: output,
        c: input ,
        d: output,
        e: inout ,
        f: inout ,
    }
    modport mp_1 {
        a: output,
        b: input ,
        c: output,
        d: input ,
        e: inout ,
        f: inout ,
    }
}
module ModuleA (
    if_0: modport InterfaceA::mp_0,
) {
    inst if_1: InterfaceA;

    connect if_1.mp_1 <> if_0;
    connect if_1.mp_0 <> '0;
}
"#;

    let expect = r#"interface prj_InterfaceA;
    logic     a;
    logic     b;
    logic     c;
    logic     d;
    tri logic e;
    tri logic f;
    modport mp_0 (
        input  a,
        output b,
        input  c,
        output d,
        inout  e,
        inout  f
    );
    modport mp_1 (
        output a,
        input  b,
        output c,
        input  d,
        inout  e,
        inout  f
    );
endinterface
module prj_ModuleA (
    prj_InterfaceA.mp_0 if_0
);
    prj_InterfaceA if_1 ();

    always_comb begin
        if_1.a = if_0.a;
        if_0.b = if_1.b;
        if_1.c = if_0.c;
        if_0.d = if_1.d;
    end
    tran (if_1.e, if_0.e);
    tran (if_1.f, if_0.f);
    always_comb begin
        if_1.b = '0;
        if_1.d = '0;
    end
    assign if_1.e = '0;
    assign if_1.f = '0;
endmodule
//# sourceMappingURL=test.sv.map
"#;

    let metadata = Metadata::create_default("prj").unwrap();

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    assert_eq!(ret, expect);

    let code = r#"interface InterfaceB {
    var a: logic;
    var b: logic;
    var c: logic;
    var d: logic;
    modport mp_0 {
        a: input ,
        b: output,
        c: input ,
        d: output,
    }
    modport mp_1 {
        a: output,
        b: input ,
        c: output,
        d: input ,
    }
}
module ModuleB (
    if_0: modport InterfaceB::mp_0,
) {
    inst if_1: InterfaceB;

    always_comb {
        if_1.mp_1 <> if_0;
    }
    always_comb {
        if_1.mp_0 <> '0;
    }
}
"#;

    let expect = r#"interface prj_InterfaceB;
    logic a;
    logic b;
    logic c;
    logic d;
    modport mp_0 (
        input  a,
        output b,
        input  c,
        output d
    );
    modport mp_1 (
        output a,
        input  b,
        output c,
        input  d
    );
endinterface
module prj_ModuleB (
    prj_InterfaceB.mp_0 if_0
);
    prj_InterfaceB if_1 ();

    always_comb begin
        begin
            if_1.a = if_0.a;
            if_0.b = if_1.b;
            if_1.c = if_0.c;
            if_0.d = if_1.d;
        end
    end
    always_comb begin
        begin
            if_1.b = '0;
            if_1.d = '0;
        end
    end
endmodule
//# sourceMappingURL=test.sv.map
"#;

    let metadata = Metadata::create_default("prj").unwrap();

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    assert_eq!(ret, expect);

    let code = r#"proto package ProtPkg {
    const COMMAND_WIDTH: u32;
    enum Command: logic<COMMAND_WIDTH> {
        WRITE,
        READ,
    }
}
package Pkg::<W: u32> {
    const COMMAND_WIDTH: u32 = W;

    enum Command: logic<COMMAND_WIDTH> {
        WRITE,
        READ,
    }
}
interface InterfaceC::<PKG: ProtPkg> {
    var valid  : logic;
    var command: PKG::Command;
    modport mp {
        valid  : output,
        command: output,
    }
}
alias interface AliasInterfaceC = InterfaceC::<Pkg::<2>>;
module ModuleC (
    a_if: modport InterfaceC::<Pkg::<2>>::mp,
    b_if: modport AliasInterfaceC::mp       ,
) {
    inst c_if: InterfaceC::<Pkg::<2>>;
    inst d_if: AliasInterfaceC;

    connect a_if    <> 0;
    connect b_if    <> 0;
    connect c_if.mp <> 0;
    connect d_if.mp <> 0;
}
"#;

    let expect = r#"

package prj___Pkg__2;
    localparam int unsigned COMMAND_WIDTH = 2;

    typedef enum logic [COMMAND_WIDTH-1:0] {
        Command_WRITE,
        Command_READ
    } Command;
endpackage
interface prj___InterfaceC____Pkg__2;
    logic                 valid  ;
    prj___Pkg__2::Command command;
    modport mp (
        output valid  ,
        output command
    );
endinterface


module prj_ModuleC (
    prj___InterfaceC____Pkg__2.mp a_if,
    prj___InterfaceC____Pkg__2.mp b_if
);
    prj___InterfaceC____Pkg__2 c_if ();
    prj___InterfaceC____Pkg__2 d_if ();

    always_comb begin
        a_if.valid   = 0;
        a_if.command = prj___Pkg__2::Command'(0);
    end
    always_comb begin
        b_if.valid   = 0;
        b_if.command = prj___Pkg__2::Command'(0);
    end
    always_comb begin
        c_if.valid   = 0;
        c_if.command = prj___Pkg__2::Command'(0);
    end
    always_comb begin
        d_if.valid   = 0;
        d_if.command = prj___Pkg__2::Command'(0);
    end
endmodule
//# sourceMappingURL=test.sv.map
"#;

    let metadata = Metadata::create_default("prj").unwrap();

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    println!("ret\n{}exp\n{}", ret, expect);
    assert_eq!(ret, expect);

    let code = r#"proto package ProtoPkg {
  enum Foo {
    FOO
  }
  type Bar;
}
package Pkg::<W: u32> for ProtoPkg {
  enum Foo {
    FOO
  }
  type Bar = logic<W>;
}
interface IfA::<PKG: ProtoPkg> {
  import PKG::*;
  var foo: Foo;
  var bar: Bar;
  modport mp {
    foo: output,
    bar: output,
  }
}
module ModuleA {
  inst if_a: IfA::<Pkg::<32>>;
  connect if_a.mp <> 0;
}
"#;

    let expect = r#"

package prj___Pkg__32;
    typedef enum logic [1-1:0] {
        Foo_FOO
    } Foo;
    typedef logic [32-1:0] Bar;
endpackage
interface prj___IfA____Pkg__32;
    import prj___Pkg__32::*;

    prj___Pkg__32::Foo foo;
    prj___Pkg__32::Bar bar;
    modport mp (
        output foo,
        output bar
    );
endinterface
module prj_ModuleA;
    prj___IfA____Pkg__32 if_a     ();
    always_comb begin
        if_a.foo = prj___Pkg__32::Foo'(0);
        if_a.bar = prj___Pkg__32::Bar'(0);
    end
endmodule
//# sourceMappingURL=test.sv.map
"#;

    let metadata = Metadata::create_default("prj").unwrap();

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    println!("ret\n{}exp\n{}", ret, expect);
    assert_eq!(ret, expect);
}

#[test]
fn inst_module_givne_via_package() {
    let code = r#"proto module ProtoModuleA;
module ModuleA for ProtoModuleA {
}
proto package ProtoPkgA {
    alias module InstModule: ProtoModuleA;
}
package PkgA::<M: ProtoModuleA> {
    alias module InstModule = M;
}
module ModuleB {
    inst u: PkgA::<ModuleA>::InstModule;
}
"#;
    let expect = r#"
module prj_ModuleA;
endmodule


package prj___PkgA__ModuleA;


endpackage
module prj_ModuleB;
    prj_ModuleA u ();
endmodule
//# sourceMappingURL=test.sv.map
"#;

    let metadata = Metadata::create_default("prj").unwrap();

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    assert_eq!(ret, expect);

    let code = r#"
proto module a_proto_module;

module a_module for a_proto_module {
}
proto package b_proto_pkg {
    alias module A_MODULE: a_proto_module;
}
package b_pkg::<MOD: a_proto_module> for b_proto_pkg {
    alias module A_MODULE = MOD;
}
proto package c_proto_pkg {
    alias module A_MODULE: a_proto_module;
}
package c_pkg::<MOD: a_proto_module> for c_proto_pkg {
    alias module A_MODULE = MOD;
}
module d_module::<PKG: b_proto_pkg> {
    import PKG::*;
    inst u: A_MODULE;
}
module e_module::<PKG: c_proto_pkg> {
    import PKG::*;
    alias package B_PKG = b_pkg::<A_MODULE>;
    inst u: d_module::<B_PKG>;
}
alias package C_PKG    = c_pkg::<a_module>;
alias module  E_MODULE = e_module::<C_PKG>;
"#;

    let expect = r#"

module prj_a_module;
endmodule


package prj___b_pkg__a_module;


endpackage


package prj___c_pkg__a_module;


endpackage
module prj___d_module____b_pkg__a_module;
    import prj___b_pkg__a_module::*;

    prj_a_module u ();
endmodule
module prj___e_module____c_pkg__a_module;
    import prj___c_pkg__a_module::*;



    prj___d_module____b_pkg__a_module u ();
endmodule


//# sourceMappingURL=test.sv.map
"#;

    let metadata = Metadata::create_default("prj").unwrap();

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    println!("ret\n{}exp\n{}", ret, expect);
    assert_eq!(ret, expect);
}

#[test]
fn expand_modport() {
    let code = r#"
proto package ProtoPkgA {
    enum Command {
        WRITE,
        READ
    }
}
package PkgA for ProtoPkgA {
    enum Command {
        WRITE,
        READ
    }
}
interface InterfaceA::<PKG: ProtoPkgA> {
    var ready  : logic;
    var valid  : logic;
    var command: PKG::Command;

    modport master {
        ready  : input ,
        valid  : output,
        command: output,
    }

    modport slave {
        ready  : output,
        valid  : input ,
        command: input ,
    }
}
#[expand(modport)]
module ModuleA::<X: u32, Y: u32> (
    a_if: modport InterfaceA::<PkgA>::master[X, Y],
    b_if: modport InterfaceA::<PkgA>::slave [X, Y],
) {
    for i in 0..X: g {
        for j in 0..Y: g {
            connect a_if[i][j] <> b_if[i][j];
        }
    }
}
module ModuleB {
    inst a_if: InterfaceA::<PkgA>[1, 2];
    inst b_if: InterfaceA::<PkgA>[1, 2];

    for i in 0..1: g {
        for j in 0..2: g {
            always_comb {
                a_if[i][j].ready = '0;
            }

            always_comb {
                b_if[i][j].valid   = '0;
                b_if[i][j].command = 0 as PkgA::Command;
            }
        }
    }

    inst u: ModuleA::<1, 2> (
        a_if,
        b_if,
    );
}
"#;

    let expect = r#"

package prj_PkgA;
    typedef enum logic [1-1:0] {
        Command_WRITE,
        Command_READ
    } Command;
endpackage
interface prj___InterfaceA__PkgA;
    logic             ready  ;
    logic             valid  ;
    prj_PkgA::Command command;

    modport master (
        input  ready  ,
        output valid  ,
        output command
    );

    modport slave (
        output ready  ,
        input  valid  ,
        input  command
    );
endinterface

module prj___ModuleA__1__2 (
    input  var logic             __a_if_0_0_ready  ,
    output var logic             __a_if_0_0_valid  ,
    output var prj_PkgA::Command __a_if_0_0_command,
    input  var logic             __a_if_0_1_ready  ,
    output var logic             __a_if_0_1_valid  ,
    output var prj_PkgA::Command __a_if_0_1_command,
    output var logic             __b_if_0_0_ready  ,
    input  var logic             __b_if_0_0_valid  ,
    input  var prj_PkgA::Command __b_if_0_0_command,
    output var logic             __b_if_0_1_ready  ,
    input  var logic             __b_if_0_1_valid  ,
    input  var prj_PkgA::Command __b_if_0_1_command
);
    prj___InterfaceA__PkgA a_if [0:1-1][0:2-1] ();
    always_comb begin
        a_if[0][0].ready   = __a_if_0_0_ready  ;
        __a_if_0_0_valid   = a_if[0][0].valid  ;
        __a_if_0_0_command = a_if[0][0].command;
    end
    always_comb begin
        a_if[0][1].ready   = __a_if_0_1_ready  ;
        __a_if_0_1_valid   = a_if[0][1].valid  ;
        __a_if_0_1_command = a_if[0][1].command;
    end
    prj___InterfaceA__PkgA b_if [0:1-1][0:2-1] ();
    always_comb begin
        __b_if_0_0_ready   = b_if[0][0].ready  ;
        b_if[0][0].valid   = __b_if_0_0_valid  ;
        b_if[0][0].command = __b_if_0_0_command;
    end
    always_comb begin
        __b_if_0_1_ready   = b_if[0][1].ready  ;
        b_if[0][1].valid   = __b_if_0_1_valid  ;
        b_if[0][1].command = __b_if_0_1_command;
    end
    for (genvar i = 0; i < 1; i++) begin :g
        for (genvar j = 0; j < 2; j++) begin :g
            always_comb begin
                b_if[i][j].ready   = a_if[i][j].ready;
                a_if[i][j].valid   = b_if[i][j].valid;
                a_if[i][j].command = b_if[i][j].command;
            end
        end
    end
endmodule
module prj_ModuleB;
    prj___InterfaceA__PkgA a_if [0:1-1][0:2-1] ();
    prj___InterfaceA__PkgA b_if [0:1-1][0:2-1] ();

    for (genvar i = 0; i < 1; i++) begin :g
        for (genvar j = 0; j < 2; j++) begin :g
            always_comb begin
                a_if[i][j].ready = '0;
            end

            always_comb begin
                b_if[i][j].valid   = '0;
                b_if[i][j].command = prj_PkgA::Command'(0);
            end
        end
    end

    prj___ModuleA__1__2 u (
        .__a_if_0_0_ready   (a_if[0][0].ready  ),
        .__a_if_0_0_valid   (a_if[0][0].valid  ),
        .__a_if_0_0_command (a_if[0][0].command),
        .__a_if_0_1_ready   (a_if[0][1].ready  ),
        .__a_if_0_1_valid   (a_if[0][1].valid  ),
        .__a_if_0_1_command (a_if[0][1].command),
        .__b_if_0_0_ready   (b_if[0][0].ready  ),
        .__b_if_0_0_valid   (b_if[0][0].valid  ),
        .__b_if_0_0_command (b_if[0][0].command),
        .__b_if_0_1_ready   (b_if[0][1].ready  ),
        .__b_if_0_1_valid   (b_if[0][1].valid  ),
        .__b_if_0_1_command (b_if[0][1].command)
    );
endmodule
//# sourceMappingURL=test.sv.map
"#;

    let metadata = Metadata::create_default("prj").unwrap();

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    assert_eq!(ret, expect);
}

#[test]
fn expand_modport_in_function() {
    let code = r#"
interface InterfaceA::<W: u32> {
    var ready  : logic;
    var valid  : logic;
    var command: logic<W>;

    modport master {
        ready  : input ,
        valid  : output,
        command: output,
    }

    modport slave {
        ready  : output,
        valid  : input ,
        command: input ,
    }

    modport monitor {
        ready  : input,
        valid  : input,
        command: input,
    }
}
package PkgA::<W: u32> {
    function FuncA (
        a: modport InterfaceA::<W>::master,
        b: modport InterfaceA::<W>::slave ,
    ) {
        a <> b;
    }

    function FuncB (
        enable: input   bool                   ,
        a     : modport InterfaceA::<W>::master,
    ) {
        if enable {
            a.valid   = 1;
            a.command = 1;
        } else {
            a <> 0;
        }
    }

    function FuncC (
        a: modport InterfaceA::<W>::monitor,
    ) -> logic {
        return a.command[0];
    }
}
module ModuleA {
    inst a_if: InterfaceA::<8>;
    inst b_if: InterfaceA::<8>;

    connect a_if.slave  <> 0;
    connect b_if.master <> 0;

    always_comb {
        PkgA::<8>::FuncA(a_if, b_if);
    }

    inst c_if: InterfaceA::<8>;
    inst d_if: InterfaceA::<8>;

    connect c_if.slave  <> 0;
    connect d_if.master <> 0;

    always_comb {
        PkgA::<8>::FuncA(a: c_if, b: d_if);
    }

    inst e_if: InterfaceA::<8>;

    connect e_if.slave  <> 0;

    function get_enable(v: input u32) -> bool {
        return v != 0;
    }

    let enable: u32 = 0;
    always_comb {
        PkgA::<8>::FuncB(get_enable(enable), e_if);
    }

    var _f: logic;
    always_comb {
        _f = PkgA::<8>::FuncC(e_if);
    }
}
"#;

    let expect = r#"interface prj___InterfaceA__8;
    logic         ready  ;
    logic         valid  ;
    logic [8-1:0] command;

    modport master (
        input  ready  ,
        output valid  ,
        output command
    );

    modport slave (
        output ready  ,
        input  valid  ,
        input  command
    );

    modport monitor (
        input ready  ,
        input valid  ,
        input command
    );
endinterface
package prj___PkgA__8;
    function automatic void FuncA(
        input  var logic         __a_ready  ,
        output var logic         __a_valid  ,
        output var logic [8-1:0] __a_command,
        output var logic         __b_ready  ,
        input  var logic         __b_valid  ,
        input  var logic [8-1:0] __b_command
    ) ;
        begin
            __b_ready   = __a_ready;
            __a_valid   = __b_valid;
            __a_command = __b_command;
        end
    endfunction

    function automatic void FuncB(
        input  var logic         enable     ,
        input  var logic         __a_ready  ,
        output var logic         __a_valid  ,
        output var logic [8-1:0] __a_command
    ) ;
        if (enable) begin
            __a_valid   = 1;
            __a_command = 1;
        end else begin
            begin
                __a_valid   = 0;
                __a_command = 0;
            end
        end
    endfunction

    function automatic logic FuncC(
        input var logic         __a_ready  ,
        input var logic         __a_valid  ,
        input var logic [8-1:0] __a_command
    ) ;
        return __a_command[0];
    endfunction
endpackage
module prj_ModuleA;
    prj___InterfaceA__8 a_if ();
    prj___InterfaceA__8 b_if ();

    always_comb begin
        a_if.ready = 0;
    end
    always_comb begin
        b_if.valid   = 0;
        b_if.command = 0;
    end

    always_comb begin
        prj___PkgA__8::FuncA(a_if.ready, a_if.valid, a_if.command, b_if.ready, b_if.valid, b_if.command);
    end

    prj___InterfaceA__8 c_if ();
    prj___InterfaceA__8 d_if ();

    always_comb begin
        c_if.ready = 0;
    end
    always_comb begin
        d_if.valid   = 0;
        d_if.command = 0;
    end

    always_comb begin
        prj___PkgA__8::FuncA(
            .__a_ready   (c_if.ready  ),
            .__a_valid   (c_if.valid  ),
            .__a_command (c_if.command),
            .__b_ready   (d_if.ready  ),
            .__b_valid   (d_if.valid  ),
            .__b_command (d_if.command)
        );
    end

    prj___InterfaceA__8 e_if ();

    always_comb begin
        e_if.ready = 0;
    end

    function automatic logic get_enable(
        input var int unsigned v
    ) ;
        return v != 0;
    endfunction

    int unsigned enable; always_comb enable = 0;
    always_comb begin
        prj___PkgA__8::FuncB(get_enable(enable), e_if.ready, e_if.valid, e_if.command);
    end

    logic _f;
    always_comb begin
        _f = prj___PkgA__8::FuncC(e_if.ready, e_if.valid, e_if.command);
    end
endmodule
//# sourceMappingURL=test.sv.map
"#;

    let metadata = Metadata::create_default("prj").unwrap();

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    println!("ret\n{}exp\n{}", ret, expect);
    assert_eq!(ret, expect);

    let code = r#"
interface a_if::<T: type> {
    var ready  : logic;
    var valid  : logic;
    var payload: T    ;

    modport slave {
        ready  : output,
        valid  : input ,
        payload: input ,
    }
}

proto package b_proto_pkg {
    const WIDTH: u32;
    struct b_struct {
        b: logic<WIDTH>,
    }
}

package b_pkg::<W: u32> for b_proto_pkg {
    const WIDTH: u32 = W;
    struct b_struct {
        b: logic<WIDTH>,
    }
}

interface c_if::<B_PKG: b_proto_pkg> {
    var ready  : logic          ;
    var valid  : logic          ;
    var payload: B_PKG::b_struct;

    function connect_if(
        aif: modport a_if::<B_PKG::b_struct>::slave,
    ) {
        aif.ready = ready;
        valid     = aif.valid;
        payload.b = aif.payload.b;
    }

    modport master {
        ready     : input ,
        valid     : output,
        payload   : output,
        connect_if: import,
    }
}

module d_module {
    alias package PKG = b_pkg::<32>;

    inst aif: a_if::<PKG::b_struct>   ;
    inst bif: c_if::<PKG>             ;
    inst cif: a_if::<PKG::b_struct>[1];
    inst dif: c_if::<PKG>[1]          ;
    inst eif: a_if::<PKG::b_struct>[1];
    inst fif: c_if::<PKG>[1]          ;

    always_comb {
        aif.valid     = '0;
        aif.payload.b = '0;
    }

    always_comb {
        bif.ready = '0;
        bif.connect_if(aif);
    }

    always_comb {
        cif[0].valid     = '0;
        cif[0].payload.b = '0;
    }

    always_comb {
        dif[0].ready = '0;
        dif[0].connect_if(aif: cif[0]);
    }

    always_comb {
        eif[0].valid     = '0;
        eif[0].payload.b = '0;
    }

    always_comb {
        fif[0].ready = '0;
        fif[0].connect_if(eif[0]);
    }
}

module e_module (
    aif: modport a_if::<b_pkg::<32>::b_struct>::slave   ,
    bif: modport c_if::<b_pkg::<32>>::master            ,
    cif: modport a_if::<b_pkg::<32>::b_struct>::slave[1],
    dif: modport c_if::<b_pkg::<32>>::master[1]         ,
    eif: modport a_if::<b_pkg::<32>::b_struct>::slave[1],
    fif: modport c_if::<b_pkg::<32>>::master[1]         ,
) {
    always_comb {
        bif.connect_if(aif);
    }

    always_comb {
        dif[0].connect_if(aif: cif[0]);
    }

    always_comb {
        fif[0].connect_if(eif[0]);
    }
}
"#;

    let expect = r#"interface prj___a_if____b_pkg__32_b_struct;
    logic                     ready  ;
    logic                     valid  ;
    prj___b_pkg__32::b_struct payload;

    modport slave (
        output ready  ,
        input  valid  ,
        input  payload
    );
endinterface


package prj___b_pkg__32;
    localparam int unsigned WIDTH = 32;
    typedef struct packed {
        logic [WIDTH-1:0] b;
    } b_struct;
endpackage

interface prj___c_if____b_pkg__32;
    logic                         ready        ;
    logic                         valid        ;
    prj___b_pkg__32::b_struct     payload      ;

    function automatic void connect_if(
        output var logic                     __aif_ready  ,
        input  var logic                     __aif_valid  ,
        input  var prj___b_pkg__32::b_struct __aif_payload
    ) ;
        __aif_ready = ready;
        valid       = __aif_valid;
        payload.b   = __aif_payload.b;
    endfunction

    modport master (
        input  ready     ,
        output valid     ,
        output payload   ,
        import connect_if
    );
endinterface

module prj_d_module;


    prj___a_if____b_pkg__32_b_struct aif ();
    prj___c_if____b_pkg__32          bif ();
    prj___a_if____b_pkg__32_b_struct cif [0:1-1] ();
    prj___c_if____b_pkg__32          dif [0:1-1] ();
    prj___a_if____b_pkg__32_b_struct eif [0:1-1] ();
    prj___c_if____b_pkg__32          fif [0:1-1] ();

    always_comb begin
        aif.valid     = '0;
        aif.payload.b = '0;
    end

    always_comb begin
        bif.ready      = '0;
        bif.connect_if(aif.ready, aif.valid, aif.payload);
    end

    always_comb begin
        cif[0].valid     = '0;
        cif[0].payload.b = '0;
    end

    always_comb begin
        dif[0].ready      = '0;
        dif[0].connect_if(
            .__aif_ready   (cif[0].ready  ),
            .__aif_valid   (cif[0].valid  ),
            .__aif_payload (cif[0].payload)
        );
    end

    always_comb begin
        eif[0].valid     = '0;
        eif[0].payload.b = '0;
    end

    always_comb begin
        fif[0].ready      = '0;
        fif[0].connect_if(eif[0].ready, eif[0].valid, eif[0].payload);
    end
endmodule

module prj_e_module (
    prj___a_if____b_pkg__32_b_struct.slave aif        ,
    prj___c_if____b_pkg__32.master         bif        ,
    prj___a_if____b_pkg__32_b_struct.slave cif [0:1-1],
    prj___c_if____b_pkg__32.master         dif [0:1-1],
    prj___a_if____b_pkg__32_b_struct.slave eif [0:1-1],
    prj___c_if____b_pkg__32.master         fif [0:1-1]
);
    always_comb begin
        bif.connect_if(aif.ready, aif.valid, aif.payload);
    end

    always_comb begin
        dif[0].connect_if(
            .__aif_ready   (cif[0].ready  ),
            .__aif_valid   (cif[0].valid  ),
            .__aif_payload (cif[0].payload)
        );
    end

    always_comb begin
        fif[0].connect_if(eif[0].ready, eif[0].valid, eif[0].payload);
    end
endmodule
//# sourceMappingURL=test.sv.map
"#;

    let metadata = Metadata::create_default("prj").unwrap();

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    println!("ret\n{}exp\n{}", ret, expect);
    assert_eq!(ret, expect);
}

#[test]
fn serialize_array_interface() {
    let code = r#"
interface InterfaceA {
    var a: logic;
    modport mp {
        a: input,
    }
}
module ModuleA (
    if_a: modport InterfaceA::mp
) {}
module ModuleB (
    if_a: modport InterfaceA::mp[4]
) {}
module ModuleC (
    if_a: modport InterfaceA::mp[3, 4]
) {}
module ModuleD (
    if_a: modport InterfaceA::mp[2, 3, 4]
) {}
module ModuleE (
    if_a: modport InterfaceA::mp[2]
) {}
module ModuleF {
    inst if_a: InterfaceA[2, 3, 4];

    inst u_a: ModuleA (
        if_a: if_a[1][2][3]
    );
    inst u_b: ModuleB (
        if_a: if_a[1][2]
    );
    inst u_c: ModuleC (
        if_a: if_a[1]
    );
    inst u_d: ModuleD (
        if_a: if_a
    );
    inst u_e0: ModuleE (
        if_a: if_a[1][2][2:3]
    );
    inst u_e1: ModuleE (
        if_a: if_a[1][2][2+:2]
    );
    inst u_e2: ModuleE (
        if_a: if_a[1][2][3-:2]
    );
    inst u_e3: ModuleE (
        if_a: if_a[1][2][1 step 2]
    );
}
"#;

    let expect = r#"interface prj_InterfaceA;
    logic a;
    modport mp (
        input a
    );
endinterface
module prj_ModuleA (
    prj_InterfaceA.mp if_a
);
endmodule
module prj_ModuleB (
    prj_InterfaceA.mp if_a [0:4-1]
);
endmodule
module prj_ModuleC (
    prj_InterfaceA.mp if_a [0:(3)*(4)-1]
);
endmodule
module prj_ModuleD (
    prj_InterfaceA.mp if_a [0:(2)*(3)*(4)-1]
);
endmodule
module prj_ModuleE (
    prj_InterfaceA.mp if_a [0:2-1]
);
endmodule
module prj_ModuleF;
    prj_InterfaceA if_a [0:(2)*(3)*(4)-1] ();

    prj_ModuleA u_a (
        .if_a (if_a[(1)*(3)*(4)+(2)*(4)+(3)])
    );
    prj_ModuleB u_b (
        .if_a (if_a[(1)*(3)*(4)+(2)*(4):(1)*(3)*(4)+((2)+1)*(4)-1])
    );
    prj_ModuleC u_c (
        .if_a (if_a[(1)*(3)*(4):((1)+1)*(3)*(4)-1])
    );
    prj_ModuleD u_d (
        .if_a (if_a)
    );
    prj_ModuleE u_e0 (
        .if_a (if_a[(1)*(3)*(4)+(2)*(4)+(2):(1)*(3)*(4)+(2)*(4)+(3)])
    );
    prj_ModuleE u_e1 (
        .if_a (if_a[(1)*(3)*(4)+(2)*(4)+(2):(1)*(3)*(4)+(2)*(4)+(2)+(2)-1])
    );
    prj_ModuleE u_e2 (
        .if_a (if_a[(1)*(3)*(4)+(2)*(4)+(3):(1)*(3)*(4)+(2)*(4)+(3)-(2)+1])
    );
    prj_ModuleE u_e3 (
        .if_a (if_a[(1)*(3)*(4)+(2)*(4)+(1)*(2):(1)*(3)*(4)+(2)*(4)+((1)+1)*(2)-1])
    );
endmodule
//# sourceMappingURL=test.sv.map
"#;

    let mut metadata = Metadata::create_default("prj").unwrap();

    metadata.build.flatten_array_interface = true;

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    assert_eq!(ret, expect);
}

#[test]
fn generic_function_call_in_function() {
    let code = r#"
proto package ProtoPkg {
    type T;
}
package Pkg::<W: u32> for ProtoPkg {
    type T = logic<W>;
}
module ModuleA::<PKG: ProtoPkg> {
    function FuncA::<T: type>() -> T {
        return 0 as T;
    }
    function FuncB() -> u32 {
        return FuncA::<PKG::T>();
    }
}
module ModuleB {
    inst u: ModuleA::<Pkg::<8>>;
}
"#;

    let expect = r#"

package prj___Pkg__8;
    typedef logic [8-1:0] T;
endpackage
module prj___ModuleA____Pkg__8;
    function automatic prj___Pkg__8::T __FuncA____Pkg__8_T() ;
        return prj___Pkg__8::T'(0);
    endfunction
    function automatic int unsigned FuncB() ;
        return __FuncA____Pkg__8_T();
    endfunction
endmodule
module prj_ModuleB;
    prj___ModuleA____Pkg__8 u ();
endmodule
//# sourceMappingURL=test.sv.map
"#;

    let metadata = Metadata::create_default("prj").unwrap();

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    println!("ret\n{}\nexp\n{}", ret, expect);
    assert_eq!(ret, expect);
}

#[test]
fn hashed_mangled_name() {
    let code = r#"
package PkgA::<
    A: u32,
    B: u32,
    C: u32,
    D: u32,
> {
    const V: u32 = A + B + C + D;
}
module ModuleA {
    function FuncA::<V: u32>() -> u32 {
        return V;
    }
    let _a: u32 = FuncA::<PkgA::<0, 1, 2, 3>::V>();
    let _b: u32 = FuncA::<PkgA::<0, 1, 2, 3>::V>();
    let _c: u32 = FuncA::<PkgA::<4, 5, 6, 7>::V>();
}
"#;

    let expect = r#"// __PkgA__0__1__2__3
package prj___PkgA__3894375d1deadabb;
    localparam int unsigned V = 0 + 1 + 2 + 3;
endpackage
// __PkgA__4__5__6__7
package prj___PkgA__c10f54f86dbec958;
    localparam int unsigned V = 4 + 5 + 6 + 7;
endpackage
module prj_ModuleA;
    // __FuncA____PkgA__0__1__2__3_V
    function automatic int unsigned __FuncA__830b4abf8aba07ce() ;
        return prj___PkgA__3894375d1deadabb::V;
    endfunction
    // __FuncA____PkgA__4__5__6__7_V
    function automatic int unsigned __FuncA__e5a0d24d19f5a43d() ;
        return prj___PkgA__c10f54f86dbec958::V;
    endfunction
    int unsigned _a; always_comb _a = __FuncA__830b4abf8aba07ce();
    int unsigned _b; always_comb _b = __FuncA__830b4abf8aba07ce();
    int unsigned _c; always_comb _c = __FuncA__e5a0d24d19f5a43d();
endmodule
//# sourceMappingURL=test.sv.map
"#;

    let mut metadata = Metadata::create_default("prj").unwrap();

    metadata.build.hashed_mangled_name = true;

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    println!("ret\n{}\nexp\n{}", ret, expect);
    assert_eq!(ret, expect);

    let code = r#"
proto package a_proto_pkg {
  const A: u32;
}
package a_pkg::<V: u32> for a_proto_pkg {
  const A: u32 = V;
}
module b_module::<PKG: a_proto_pkg> {
  import PKG::*;
}
module c_module::<PKG: a_proto_pkg> {
  inst u: b_module::<PKG>;
}
alias package pkg = a_pkg::<32>;
alias module mod = c_module::<pkg>;
"#;

    let expect = r#"
// __a_pkg__32

package prj___a_pkg__805a71dc85438e33;
    localparam int unsigned A = 32;
endpackage
// __b_module____a_pkg__32
module prj___b_module__dc41fc063ac19318;
    import prj___a_pkg__805a71dc85438e33::*;

endmodule
// __c_module____a_pkg__32
module prj___c_module__dc41fc063ac19318;
    prj___b_module__dc41fc063ac19318 u ();
endmodule


//# sourceMappingURL=test.sv.map
"#;

    let mut metadata = Metadata::create_default("prj").unwrap();

    metadata.build.hashed_mangled_name = true;

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    println!("ret\n{}\nexp\n{}", ret, expect);
    assert_eq!(ret, expect);
}

#[test]
fn struct_member_as_generic_arg() {
    let code = r#"
module ModuleA {
    struct Foo {
        foo: logic,
    }
    struct Bar {
        bar: Foo,
    }
    struct Baz {
        baz: Bar,
    }

    const QUX: Baz = Baz'{ baz: Bar'{ bar: Foo'{ foo: 0 } } };

    function Func::<bar: Bar> -> logic {
        return bar.bar.foo;
    }

    let _a: logic = Func::<QUX.baz>();
}
"#;

    let expect = r#"module prj_ModuleA;
    typedef struct packed {
        logic foo;
    } Foo;
    typedef struct packed {
        Foo bar;
    } Bar;
    typedef struct packed {
        Bar baz;
    } Baz;

    localparam Baz QUX = Baz'{baz: Bar'{bar: Foo'{foo: 0}}};

    function automatic logic __Func__QUX_baz;
        return QUX.baz.bar.foo;
    endfunction

    logic _a; always_comb _a = __Func__QUX_baz();
endmodule
//# sourceMappingURL=test.sv.map
"#;

    let metadata = Metadata::create_default("prj").unwrap();

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    println!("ret\n{}\nexp\n{}", ret, expect);
    assert_eq!(ret, expect);

    let code = r#"
package Pkg {
    struct Foo {
        foo: logic,
    }
    struct Bar {
        bar: Foo,
    }
    struct Baz {
        baz: Bar,
    }

    const QUX: Baz = Baz'{ baz: Bar'{ bar: Foo'{ foo: 0 } } };
}
module ModuleB {
    function Func::<bar: Pkg::Bar> -> logic {
        return bar.bar.foo;
    }
    let _a: logic = Func::<Pkg::QUX.baz>();
}
"#;

    let expect = r#"package prj_Pkg;
    typedef struct packed {
        logic foo;
    } Foo;
    typedef struct packed {
        Foo bar;
    } Bar;
    typedef struct packed {
        Bar baz;
    } Baz;

    localparam Baz QUX = Baz'{baz: Bar'{bar: Foo'{foo: 0}}};
endpackage
module prj_ModuleB;
    function automatic logic __Func__Pkg_QUX_baz;
        return prj_Pkg::QUX.baz.bar.foo;
    endfunction
    logic _a; always_comb _a = __Func__Pkg_QUX_baz();
endmodule
//# sourceMappingURL=test.sv.map
"#;

    let metadata = Metadata::create_default("prj").unwrap();

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    println!("ret\n{}\nexp\n{}", ret, expect);
    assert_eq!(ret, expect);

    let code = r#"package foo_pkg::<V: u32> {
    struct foo_struct {
      foo: u32,
    }

    const FOO: foo_struct = foo_struct'{ foo: V };
}
package bar_pkg::<V: u32> {
   const BAR: u32 = V;
}

alias package FooPkg = foo_pkg::<32>;
alias package BarPkg = bar_pkg::<FooPkg::FOO.foo>;
"#;

    let expect = r#"package prj___foo_pkg__32;
    typedef struct packed {
        int unsigned foo;
    } foo_struct;

    localparam foo_struct FOO = foo_struct'{foo: 32};
endpackage
package prj___bar_pkg____foo_pkg__32_FOO_foo;
    localparam int unsigned BAR = prj___foo_pkg__32::FOO.foo;
endpackage


//# sourceMappingURL=test.sv.map
"#;

    let metadata = Metadata::create_default("prj").unwrap();

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    println!("ret\n{}\nexp\n{}", ret, expect);
    assert_eq!(ret, expect);
}

#[test]
fn emmit_inst_param_port_item_assigned_by_name() {
    let code = r#"
module ModuleA #(
    param A: u32 = 32,
) (
    b: input logic,
) {
}
module ModuleB::<A: u32, b: u32> {
    inst a: ModuleA #(A) (b);
}
module ModuleC {
  inst b: ModuleB::<1, 2>;
}
"#;

    let expect = r#"module prj_ModuleA #(
    parameter int unsigned A = 32
) (
    input var logic b
);
endmodule
module prj___ModuleB__1__2;
    prj_ModuleA #(
        .A (1)
    ) a (
        .b (2)
    );
endmodule
module prj_ModuleC;
    prj___ModuleB__1__2 b ();
endmodule
//# sourceMappingURL=test.sv.map
"#;

    let metadata = Metadata::create_default("prj").unwrap();

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    println!("ret\n{}exp\n{}", ret, expect);
    assert_eq!(ret, expect);
}

#[test]
fn emit_generic_function_with_imported_proto_item() {
    let code = r#"proto package ProtoPkgA {
    type A;
}
package PkgA::<W: u32> for ProtoPkgA {
    type A = logic<W>;
}
module ModuleA::<PKG: ProtoPkgA> {
    import PKG::*;
    function func::<T: type>() -> u32 {
        return $bits(T);
    }
    let _w: u32 = func::<A>();
}
module ModuleB {
    inst u_a: ModuleA::<PkgA::<32>>;
}
"#;

    let expect = r#"

package prj___PkgA__32;
    typedef logic [32-1:0] A;
endpackage
module prj___ModuleA____PkgA__32;
    import prj___PkgA__32::*;

    function automatic int unsigned __func____PkgA__32_A() ;
        return $bits(prj___PkgA__32::A);
    endfunction
    int unsigned _w; always_comb _w = __func____PkgA__32_A();
endmodule
module prj_ModuleB;
    prj___ModuleA____PkgA__32 u_a ();
endmodule
//# sourceMappingURL=test.sv.map
"#;

    let metadata = Metadata::create_default("prj").unwrap();

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    println!("ret\n{}exp\n{}", ret, expect);
    assert_eq!(ret, expect);
}

#[test]
fn boolean_literal_as_generic_arg() {
    let code = r#"module ModuleA::<A: bool> {
    let _a: bool = A;
}
module ModuleB {
    inst u0: ModuleA::<true> ;
    inst u1: ModuleA::<false>;
}
"#;

    let expect = r#"module prj___ModuleA__true;
    logic _a; always_comb _a = 1'b1;
endmodule
module prj___ModuleA__false;
    logic _a; always_comb _a = 1'b0;
endmodule
module prj_ModuleB;
    prj___ModuleA__true  u0 ();
    prj___ModuleA__false u1 ();
endmodule
//# sourceMappingURL=test.sv.map
"#;

    let metadata = Metadata::create_default("prj").unwrap();

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    println!("ret\n{}exp\n{}", ret, expect);
    assert_eq!(ret, expect);
}

#[test]
fn import_declaration_with_generic_package() {
    let code = r#"package PkgA::<V: u32> {
  const A: u32 = V;
}
module ModuleA {
  import PkgA::<1>::*;
  const A: u32 = PkgA::<2>::A;
}
"#;

    let expect = r#"package prj___PkgA__1;
    localparam int unsigned A = 1;
endpackage
package prj___PkgA__2;
    localparam int unsigned A = 2;
endpackage
module prj_ModuleA;
    import prj___PkgA__1::*;

    localparam int unsigned A = prj___PkgA__2::A;
endmodule
//# sourceMappingURL=test.sv.map
"#;

    let metadata = Metadata::create_default("prj").unwrap();

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    println!("ret\n{}exp\n{}", ret, expect);
    assert_eq!(ret, expect);
}

#[test]
fn bind_declaration() {
    let code = r#"
proto package ProtoPkg {
}

package Pkg::<A_VALUE: u32> for ProtoPkg {
}

module ModuleA::<PKG: ProtoPkg> {}

module ModuleB::<PKG: ProtoPkg> {}

alias module mod = ModuleC::<32>;

module ModuleC::<A_VALUE: u32> {
  alias package pkg = Pkg::<A_VALUE>;

  bind ModuleA::<pkg> <- u: ModuleB::<pkg>;
}
"#;

    let expect = r#"

package prj___Pkg__32;
endpackage

module prj___ModuleA____Pkg__32;
endmodule

module prj___ModuleB____Pkg__32;
endmodule


module prj___ModuleC__32;


    bind prj___ModuleA____Pkg__32 prj___ModuleB____Pkg__32 u ();
endmodule
//# sourceMappingURL=test.sv.map
"#;

    let metadata = Metadata::create_default("prj").unwrap();

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    println!("ret\n{}exp\n{}", ret, expect);
    assert_eq!(ret, expect);
}

#[test]
fn package_alias_defined_in_module() {
    let code = r#"
proto package a_proto_pkg {
    struct a_struct {
        a: u32,
    }
    const A: a_struct;
}
package a_pkg::<A_VALUE: u32> for a_proto_pkg {
    struct a_struct {
        a: u32,
    }
    const A: a_struct = a_struct'{ a: A_VALUE };
}

proto package b_proto_pkg {
    const B: u32;
}
package b_pkg::<B_VALUE: u32> for b_proto_pkg {
    const B: u32 = B_VALUE;
}

module c_module::<PKG: b_proto_pkg> {
}

module d_module::<A_PKG: a_proto_pkg> {
    import A_PKG::*;
    alias package B_PKG = b_pkg::<A.a>;
    inst u: c_module::<B_PKG>;
}

alias package A_PKG    = a_pkg::<32>;
alias module  D_MODULE = d_module::<A_PKG>;
"#;

    let expect = r#"

package prj___a_pkg__32;
    typedef struct packed {
        int unsigned a;
    } a_struct;
    localparam a_struct A = a_struct'{a: 32};
endpackage


package prj___b_pkg____a_pkg__32_A_a;
    localparam int unsigned B = prj___a_pkg__32::A.a;
endpackage

module prj___c_module____b_pkg____a_pkg__32_A_a;
endmodule

module prj___d_module____a_pkg__32;
    import prj___a_pkg__32::*;



    prj___c_module____b_pkg____a_pkg__32_A_a u ();
endmodule


//# sourceMappingURL=test.sv.map
"#;

    let metadata = Metadata::create_default("prj").unwrap();

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    println!("ret\n{}exp\n{}", ret, expect);
    assert_eq!(ret, expect);
}

#[test]
fn package_alias_defined_in_package() {
    let code = r#"
proto package a_proto_pkg {
    const A_TYPE: type;
}
package a_pkg::<a_type: type> for a_proto_pkg {
    const A_TYPE: type = a_type;
}
proto package b_proto_pkg {
    alias package A_PKG: a_proto_pkg;
}
package b_pkg::<a_type: type> for b_proto_pkg {
    alias package A_PKG = a_pkg::<a_type>;
}
module c_module::<B_PKG: b_proto_pkg> {
    import B_PKG::*;
    let _d: B_PKG::A_PKG::A_TYPE = 0;
    let _e: A_PKG::A_TYPE        = 0;
}
alias package B_PKG    = b_pkg::<bool>;
alias module  C_MODULE = c_module::<B_PKG>;
"#;

    let expect = r#"

package prj___a_pkg__bool;
    localparam type A_TYPE = logic;
endpackage


package prj___b_pkg__bool;


endpackage
module prj___c_module____b_pkg__bool;
    import prj___b_pkg__bool::*;

    prj___a_pkg__bool::A_TYPE _d; always_comb _d = 0;
    prj___a_pkg__bool::A_TYPE _e; always_comb _e = 0;
endmodule


//# sourceMappingURL=test.sv.map
"#;

    let metadata = Metadata::create_default("prj").unwrap();

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    println!("ret\n{}exp\n{}", ret, expect);
    assert_eq!(ret, expect);
}
