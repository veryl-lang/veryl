module veryl_testcase_Module12_1 (
    input logic i_clk  ,
    input logic i_rst_n
);
    logic a;
    logic b;
    logic c;

    always_ff @ (posedge i_clk, negedge i_rst_n) begin
        if (!i_rst_n) begin
            c <= 0;
        end else begin
            c <= ~a;
        end
    end

    always_ff @ (posedge i_clk, negedge i_rst_n) begin
        if (!i_rst_n) begin
            a <= 0;
        end else begin
            a <= ~a;
        end
    end

    always_ff @ (posedge i_clk) begin
        b <= a;
    end
endmodule

module veryl_testcase_Module12_2 (
    input logic i_clk   ,
    input logic i_clk_p ,
    input logic i_clk_n ,
    input logic i_rst_n ,
    input logic i_rst_ah,
    input logic i_rst_al,
    input logic i_rst_sh,
    input logic i_rst_sl
);
    logic a ;
    logic aa;
    logic b ;
    always_comb b = 1;
    logic c ;
    always_comb c = 1;

    // always_ff declaration with default polarity
    always_ff @ (posedge i_clk, negedge i_rst_n) begin
        if (!i_rst_n) begin
            a <= 1'b0;
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
    always_ff @ (posedge i_clk_p, posedge i_rst_ah) begin
        if (i_rst_ah) begin
            a <= 1'b0;
        end else begin
            a <= c[5:0];
        end
    end
    always_ff @ (negedge i_clk_n, negedge i_rst_al) begin
        if (!i_rst_al) begin
            a <= 1'b0;
        end else begin
            a <= c[5:0];
        end
    end
    always_ff @ (posedge i_clk_p) begin
        if (i_rst_sh) begin
            a <= 1'b0;
        end else begin
            a <= c[5:0];
        end
    end
    always_ff @ (negedge i_clk_n) begin
        if (!i_rst_sl) begin
            a <= 1'b0;
        end else begin
            a <= c[5:0];
        end
    end

    // if_reset with loop
    logic [10-1:0] d;
    for (genvar i = 0; i < 10; i++) begin :g
        always_ff @ (posedge i_clk, negedge i_rst_n) begin
            if (!i_rst_n) begin
                d[i] <= i;
            end
        end
    end

    // if_reset with loop
    logic [10-1:0] e;
    always_ff @ (posedge i_clk, negedge i_rst_n) begin
        if (!i_rst_n) begin
            for (int unsigned i = 0; i < 10; i++) begin
                e[i] <= i;
            end
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
//# sourceMappingURL=../map/testcases/sv/12_always.sv.map
