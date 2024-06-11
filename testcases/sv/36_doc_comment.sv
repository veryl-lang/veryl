/// Test module for doc comment
///
/// * list item0
/// * list item1
///
/// ```wavedrom
/// {signal: [
///   {name: 'clk', wave: 'p.....|...'},
///   {name: 'dat', wave: 'x.345x|=.x', data: ['head', 'body', 'tail', 'data']},
///   {name: 'req', wave: '0.1..0|1.0'},
///   {},
///   {name: 'ack', wave: '1.....|01.'}
///
/// ]}
/// ```
///
module veryl_testcase_Module36 #(
    /// Data width
    parameter  int unsigned ParamA = 1,
    localparam int unsigned ParamB = 1
) (
    input  logic              i_clk  , /// Clock
    input  logic              i_rst_n, /// Reset
    input  logic [ParamA-1:0] i_data , /// Data input
    output logic [ParamA-1:0] o_data  /// Data output
);
    always_comb o_data = 0;
endmodule

/// Test interface for doc comment
///
/// * list item0
/// * list item1
interface veryl_testcase_Interface36 #(
    parameter  int unsigned ParamA = 1, /// Data width
    localparam int unsigned ParamB = 1
);
endinterface

/// Test package for doc comment
///
/// * list item0
/// * list item1
package veryl_testcase_Package36;
endpackage
//# sourceMappingURL=../map/testcases/sv/36_doc_comment.sv.map
