module veryl_testcase_Module12_1 (
    input var logic i_clk  ,
    input var logic i_rst_n
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
    input var logic i_clk   ,
    input var logic i_clk_p ,
    input var logic i_clk_n ,
    input var logic i_rst_n ,
    input var logic i_rst_ah,
    input var logic i_rst_al,
    input var logic i_rst_sh,
    input var logic i_rst_sl
);
    logic          a0 ;
    logic          a1 ;
    logic          a2 ;
    logic          a3 ;
    logic          a4 ;
    logic          a5 ;
    logic          a  ;
    logic          aa ;
    logic          aaa;
    logic          b  ; always_comb b   = 1;
    logic [10-1:0] c  ; always_comb c   = 1;

    // always_ff declaration with default polarity
    always_ff @ (posedge i_clk, negedge i_rst_n) begin
        if (!i_rst_n) begin
            a0 <= 1'b0;
        end else if (a) begin
            a0 <= b[0];
        end else begin
            a0 <= c[5:0];
        end
    end

    // always_ff declaration without reset
    always_ff @ (posedge i_clk) begin
        if (a) begin
            a1 <= b;
        end else begin
            a1 <= c[5:0];
        end
    end

    // always_ff declaration with specified polarity
    always_ff @ (posedge i_clk_p, posedge i_rst_ah) begin
        if (i_rst_ah) begin
            a2 <= 1'b0;
        end else begin
            a2 <= c[5:0];
        end
    end
    always_ff @ (negedge i_clk_n, negedge i_rst_al) begin
        if (!i_rst_al) begin
            a3 <= 1'b0;
        end else begin
            a3 <= c[5:0];
        end
    end
    always_ff @ (posedge i_clk_p) begin
        if (i_rst_sh) begin
            a4 <= 1'b0;
        end else begin
            a4 <= c[5:0];
        end
    end
    always_ff @ (negedge i_clk_n) begin
        if (!i_rst_sl) begin
            a5 <= 1'b0;
        end else begin
            a5 <= c[5:0];
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

    // if_reset by reset value initialized by function
    localparam logic X = $clog2(1);
    logic f;
    always_ff @ (posedge i_clk, negedge i_rst_n) begin
        if (!i_rst_n) begin
            f <= X;
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
//# sourceMappingURL=../map/12_always.sv.map
