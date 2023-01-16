module Module12 (
    input logic  i_clk,
    input logic  i_rst
) ;
    logic  a ;
    logic  aa;
    logic  b ;
    logic  c ;

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
        a    = 10;
        aa   = 10'b0;
        aa.a = 10'b01z;

        a  = 10 + 10;
        aa = 10 + 16'hffff * (3 / 4);
    end
endmodule
