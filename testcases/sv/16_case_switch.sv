module veryl_testcase_Module16;
    logic         a;
    logic         b;
    logic         x;
    always_comb x = 1;
    logic         y;
    always_comb y = 1;
    logic [3-1:0] z;
    always_comb z = 1;

    always_comb begin
        case (x) inside
            0: a = 1;
            1: a = 1;
            2: begin
                   a = 1;
                   a = 1;
                   a = 1;
               end
            3, 4   : a = 1;
            y - 1  : a = 1;
            default: a = 1;
        endcase
    end

    always_comb begin
        case (1'b1)
            z == 0: b = 1;
            z == 1: b = 1;
            z == 2: begin
                        b = 1;
                        b = 1;
                        b = 1;
                    end
            z == 3, z == 4: b = 1;
            default       : b = 1;
        endcase
    end
endmodule
//# sourceMappingURL=../map/testcases/sv/16_case_switch.sv.map
