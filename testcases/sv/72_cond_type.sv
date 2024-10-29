module veryl_testcase_Module72 (
    input logic i_clk  ,
    input logic i_rst_n
);
    logic x;
    always_comb x = 1;
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

        case (x) inside
            0: a = 1;
            default: a = 1;
        endcase

        case (x) inside
            0: b = 1;
            default: b = 1;
        endcase

        case (x) inside
            0: c = 1;
            default: c = 1;
        endcase
    end

    always_comb begin

        if (x == 0) begin
            d = 1;
        end else begin
            d = 1;
        end

        if (x == 0) begin
            e = 1;
        end else begin
            e = 1;
        end

        if (x == 0) begin
            f = 1;
        end else begin
            f = 1;
        end
    end

    always_ff @ (posedge i_clk, negedge i_rst_n) begin

        if (!i_rst_n) begin
            g <= 1;
        end else begin
            g <= 1;
        end
    end
    always_ff @ (posedge i_clk, negedge i_rst_n) begin

        if (!i_rst_n) begin
            h <= 1;
        end else begin
            h <= 1;
        end
    end
    always_ff @ (posedge i_clk, negedge i_rst_n) begin

        if (!i_rst_n) begin
            i <= 1;
        end else begin
            i <= 1;
        end
    end
endmodule
//# sourceMappingURL=../map/testcases/sv/72_cond_type.sv.map
