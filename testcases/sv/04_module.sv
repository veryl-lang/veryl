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
    parameter  int unsigned     a   = 1;
    localparam longint unsigned aa  = 1;

    // variable declaration
    logic                  a  ;
    logic [10-1:0]         aa ;
    bit   [10-1:0][10-1:0] aaa;

    // assign declaration
    assign a   = 1;
    assign aa  = 1;
    assign aaa = 1;

    // always_ff declaration
    always_ff @ (posedge i_clk, negedge i_rst_n) begin
        if (a) begin
            a <= b;
        end else if (a) begin
            a <= b[0];
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
endmodule
