// module declaration
module ModuleA #(
    // module parameter
    parameter  int unsigned a   = 1,
    localparam int unsigned aa  = 1
) (
    // module port
    input  logic [10-1:0] a  ,
    output logic [10-1:0] aa ,
    inout  logic [10-1:0] aaa
) ;
    // parameter declaration
    parameter  int unsigned     b   = 1;
    localparam longint unsigned bb  = 1;

    // variable declaration
    logic                  b  ;
    logic [10-1:0]         bb ;
    bit   [10-1:0][10-1:0] bbb;

    // assign declaration
    assign a   = 1;
    assign aa  = 1;
    assign aaa = 1;

    // assign declaration with variable declaration
    logic [10-1:0] c;
    assign c = 1;

    // always_ff declaration with default polarity
    always_ff @ (posedge i_clk, negedge i_rst) begin
        if (!i_rst) begin
            a <= b;
        end else if (a) begin
            a <= b[0];
        end else begin
            a <= c[5:0];
        end
    end

    // always_ff declaration without reset
    always_ff @ (posedge i_clk) begin
        if (a) begin
            a <= b;
        end else begin
            a <= c[5:0];
        end
    end

    // always_ff declaration with specified polarity
    always_ff @ (posedge i_clk, posedge i_rst) begin
        if (i_rst) begin
            a <= b;
        end else begin
            a <= c[5:0];
        end
    end
    always_ff @ (negedge i_clk, negedge i_rst) begin
        if (!i_rst) begin
            a <= b;
        end else begin
            a <= c[5:0];
        end
    end
    always_ff @ (posedge i_clk) begin
        if (i_rst) begin
            a <= b;
        end else begin
            a <= c[5:0];
        end
    end
    always_ff @ (negedge i_clk) begin
        if (!i_rst) begin
            a <= b;
        end else begin
            a <= c[5:0];
        end
    end

    // always_comb declaration
    always_comb begin
        a   = 10;
        aa  = 10'b0;
        aaa = 10'b01z;
        a  = 10 + 10;
        aa = 10 + 16'hffff * (3 / 4);
    end

    // module instantiation
    ModuleB #(
        .a  (a ),
        .aa (10)
    ) b (
        .a    (a  ),
        .bb   (aa ),
        .bbbb (bbb)
    );
endmodule
