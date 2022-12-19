module a #(
    parameter  int unsigned a   = 1,
    localparam int unsigned aa  = 1
) (
    input  logic [10-1:0] a  ,
    output logic [10-1:0] aa ,
    inout  logic [10-1:0] aaa
) ;
    parameter  int unsigned     a   = 1;
    localparam longint unsigned aa  = 1;

    logic                   a   ;
    logic  [10-1:0]         aa  ;
    bit    [10-1:0][10-1:0] aaa ;
    type_t aaaa [10-1:0]        ;

    always_ff @ (posedge i_clk, negedge i_rst_n) begin
        if (a) begin
            a <= b;
        end else if (a) begin
            a <= b[0];
        end else begin
            a <= c[5:0];
        end
    end

    always_comb begin
        a   = 10;
        aa  = 10'b0;
        aaa = 10'b01z;

        a  = 10 + 10;
        aa = 10 + 16'hffff * (3 / 4);
    end
endmodule
