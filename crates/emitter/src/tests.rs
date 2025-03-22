use crate::Emitter;
use std::path::PathBuf;
use veryl_analyzer::Analyzer;
use veryl_metadata::{ClockType, Metadata, ResetType};
use veryl_parser::Parser;

#[track_caller]
fn emit(metadata: &Metadata, code: &str) -> String {
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

    let mut metadata: Metadata =
        toml::from_str(&Metadata::create_default_toml("prj").unwrap()).unwrap();

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

    let mut metadata: Metadata =
        toml::from_str(&Metadata::create_default_toml("prj").unwrap()).unwrap();

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
fn omit_project_prefix() {
    let code = r#"module ModuleA {
}

package PackageA {
}

interface InterfaceA {
}
"#;

    let expect = r#"module ModuleA;
endmodule

package PackageA;
endpackage

interface InterfaceA;
endinterface
//# sourceMappingURL=test.sv.map
"#;

    let mut metadata: Metadata =
        toml::from_str(&Metadata::create_default_toml("prj").unwrap()).unwrap();

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

    let mut metadata: Metadata =
        toml::from_str(&Metadata::create_default_toml("prj").unwrap()).unwrap();

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

    let mut metadata: Metadata =
        toml::from_str(&Metadata::create_default_toml("prj").unwrap()).unwrap();

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

    let mut metadata: Metadata =
        toml::from_str(&Metadata::create_default_toml("prj").unwrap()).unwrap();

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

    let mut metadata: Metadata =
        toml::from_str(&Metadata::create_default_toml("prj").unwrap()).unwrap();

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

    let mut metadata: Metadata =
        toml::from_str(&Metadata::create_default_toml("prj").unwrap()).unwrap();

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

    let mut metadata: Metadata =
        toml::from_str(&Metadata::create_default_toml("prj").unwrap()).unwrap();

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
    let code = r#"package Pkg::<A: const> {
    function Func::<B: const> -> i32 {
        return A + B;
    }
    struct Struct::<W: cosnt> {
        foo: logic<W>,
    }
}
module Module {
    let _a: i32 = Pkg::<1>::Func::<1>();
    let _b: i32 = Pkg::<1>::Func::<2>();
    let _c: i32 = Pkg::<2>::Func::<1>();
    var _d: Pkg::<1>::Struct::<2>;
    var _e: Pkg::<2>::Struct::<1>;
    var _f: Pkg::<2>::Struct::<2>;
}
"#;

    let expect = r#"package prj___Pkg__1;
    function automatic int signed __Func__1;
        return 1 + 1;
    endfunction
    function automatic int signed __Func__2;
        return 1 + 2;
    endfunction
    typedef struct packed {
        logic [2-1:0] foo;
    } __Struct__2;
endpackage
package prj___Pkg__2;
    function automatic int signed __Func__1;
        return 2 + 1;
    endfunction
    typedef struct packed {
        logic [1-1:0] foo;
    } __Struct__1;
    typedef struct packed {
        logic [2-1:0] foo;
    } __Struct__2;
endpackage
module prj_Module;
    int signed                _a; always_comb _a = prj___Pkg__1::__Func__1();
    int signed                _b; always_comb _b = prj___Pkg__1::__Func__2();
    int signed                _c; always_comb _c = prj___Pkg__2::__Func__1();
    prj___Pkg__1::__Struct__2 _d;
    prj___Pkg__2::__Struct__1 _e;
    prj___Pkg__2::__Struct__2 _f;
endmodule
//# sourceMappingURL=test.sv.map
"#;

    let metadata: Metadata =
        toml::from_str(&Metadata::create_default_toml("prj").unwrap()).unwrap();

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

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

    let metadata: Metadata =
        toml::from_str(&Metadata::create_default_toml("prj").unwrap()).unwrap();

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
        if_1.a = if_0.a;
        if_0.b = if_1.b;
        if_1.c = if_0.c;
        if_0.d = if_1.d;
    end
    always_comb begin
        if_1.b = '0;
        if_1.d = '0;
    end
endmodule
//# sourceMappingURL=test.sv.map
"#;

    let metadata: Metadata =
        toml::from_str(&Metadata::create_default_toml("prj").unwrap()).unwrap();

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
package Pkg::<W: const> {
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
module ModuleC (
    a_if: modport InterfaceC::<Pkg::<2>>::mp,
) {
    inst b_if: InterfaceC::<Pkg::<2>>;

    connect a_if    <> 0;
    connect b_if.mp <> 0;
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
    prj___InterfaceC____Pkg__2.mp a_if
);
    prj___InterfaceC____Pkg__2 b_if ();

    always_comb begin
        a_if.valid   = 0;
        a_if.command = prj___Pkg__2::Command'(0);
    end
    always_comb begin
        b_if.valid   = 0;
        b_if.command = prj___Pkg__2::Command'(0);
    end
endmodule
//# sourceMappingURL=test.sv.map
"#;

    let metadata: Metadata =
        toml::from_str(&Metadata::create_default_toml("prj").unwrap()).unwrap();

    let ret = if cfg!(windows) {
        emit(&metadata, code).replace("\r\n", "\n")
    } else {
        emit(&metadata, code)
    };

    assert_eq!(ret, expect);
}
