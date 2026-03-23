/// Combinational pass-through.
///
/// ```wavedrom,test
/// {signal: [
///   {name: 'clk', wave: 'p.....'},
///   {name: 'din', wave: '010101'},
///   {name: 'dout', wave: '010101'}
/// ]}
/// ```
module veryl_testcase_Module84 (
    input  var logic i_clk    ,
    input  var logic i_rst_n_n,
    input  var logic i_din    ,
    output var logic o_dout   
);
    always_comb o_dout = i_din;
endmodule

/// 1-cycle delay register.
///
/// ```wavedrom,test
/// {signal: [
///   {name: 'clk',   wave: 'p........'},
///   {name: 'rst_n', wave: '0.1......'},
///   {name: 'din',   wave: '0.01.0.1.'},
///   {name: 'dout',  wave: '0...1.0.1'}
/// ]}
/// ```
module veryl_testcase_Module84_delay (
    input  var logic i_clk    ,
    input  var logic i_rst_n_n,
    input  var logic i_din    ,
    output var logic o_dout   
);
    logic r_data;

    always_ff @ (posedge i_clk, negedge i_rst_n_n) begin
        if (!i_rst_n_n) begin
            r_data <= 0;
        end else begin
            r_data <= i_din;
        end
    end

    always_comb o_dout = r_data;
endmodule

/// 4-bit up counter.
///
/// ```wavedrom,test
/// {signal: [
///   {name: 'clk',   wave: 'p.......'},
///   {name: 'rst_n', wave: '0.1.....'},
///   {name: 'count', wave: '0..=====', data: ['1', '2', '3', '4', '5']}
/// ]}
/// ```
module veryl_testcase_Module84_counter (
    input  var logic         i_clk    ,
    input  var logic         i_rst_n_n,
    output var logic [4-1:0] o_count  
);
    logic [4-1:0] r_count;

    always_ff @ (posedge i_clk, negedge i_rst_n_n) begin
        if (!i_rst_n_n) begin
            r_count <= 0;
        end else begin
            r_count <= r_count + 1;
        end
    end

    always_comb o_count = r_count;
endmodule
//# sourceMappingURL=../map/85_doc_test.sv.map
