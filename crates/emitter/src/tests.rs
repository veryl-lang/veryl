use crate::Emitter;
use std::path::PathBuf;
use veryl_analyzer::Analyzer;
use veryl_metadata::{ClockType, Metadata, ResetType};
use veryl_parser::Parser;

#[track_caller]
fn emit(metadata: &Metadata, code: &str) -> String {
    let parser = Parser::parse(&code, &"").unwrap();
    let analyzer = Analyzer::new(metadata);

    analyzer.analyze_pass1(&"prj", &code, &"", &parser.veryl);
    analyzer.analyze_pass2(&"prj", &code, &"", &parser.veryl);

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
    input logic clk_pos_clk_clk_pos  ,
    input logic rst_high_rst_rst_high
);
    prj_ModuleB u (
        .clk_pos_clk_clk_pos   (clk_pos_clk_clk_pos  ),
        .rst_high_rst_rst_high (rst_high_rst_rst_high)
    );

    logic _a;
    always_comb _a = clk_pos_clk_clk_pos;
    logic _b;
    always_comb _b = rst_high_rst_rst_high;

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
    input logic clk_pos_clk_clk_pos  ,
    input logic rst_high_rst_rst_high
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
    input logic clk_neg_clk_clk_neg,
    input logic rst_low_rst_rst_low
);
    prj_ModuleB u (
        .clk_neg_clk_clk_neg (clk_neg_clk_clk_neg),
        .rst_low_rst_rst_low (rst_low_rst_rst_low)
    );

    logic _a;
    always_comb _a = clk_neg_clk_clk_neg;
    logic _b;
    always_comb _b = rst_low_rst_rst_low;

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
    input logic clk_neg_clk_clk_neg,
    input logic rst_low_rst_rst_low
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
