module veryl_testcase_Module16;
    logic a;
    logic x;
    always_comb x = 1;
    logic y;
    always_comb y = 1;

    always_comb begin
        case (x)
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
endmodule
//# sourceMappingURL=../map/testcases/sv/16_case.sv.map
