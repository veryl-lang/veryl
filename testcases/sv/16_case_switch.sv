module veryl_testcase_Module16;
    localparam logic A = 1;
    localparam bit   B = 1;

    logic         a;
    logic         b;
    logic         c;
    logic         x; always_comb x = 1;
    logic         y; always_comb y = 1;
    logic [3-1:0] z; always_comb z = 1;

    always_comb begin
        case (x) inside
            0: a = 1; // comment
            1: a = 1; // comment
            2: begin // comment
                a = 1; // comment
                a = 1;
                a = 1;
            end //
            3, 4           : a = 1;
            [5:7          ]: a = 1;
            A - 1          : a = 1;
            10, 11, 12, 13,
            14, 15, 16, 17 : a = 1;
            default        : a = 1;
        endcase
    end

    always_comb begin
        case (y)
            0      : b = 1;
            1      : b = 1;
            2, 3   : b = 1;
            B - 1  : b = 1;
            default: b = 1;
        endcase
    end

    always_comb begin
        case (1'b1)
            z == 0: c = 1;
            z == 1: c = 1;
            z == 2: begin
                c = 1;
                c = 1;
                c = 1;
            end //
            z == 3, z == 4                                 : c = 1;
            z == 4'd05, z == 5'd06, z == 5'd07, z == 5'd08,
            z == 5'd09, z == 4'd10, z == 4'd11, z == 4'd12,
            z == 4'd13, z == 4'd14, z == 4'd15, z == 5'd16 : c = 1;
            default                                        : c = 1;
        endcase
    end
endmodule
//# sourceMappingURL=../map/16_case_switch.sv.map
