module veryl_testcase_Module53;
    typedef enum logic {
        EnumA_A
    } EnumA;

    logic _a;
    always_comb begin
        case (EnumA_A)
            /*
            */
            EnumA_A: _a = 0;
            default: _a = 1;
        endcase
    end
endmodule
//# sourceMappingURL=../map/testcases/sv/53_multiline_comment_case.sv.map
