// Parametric width inference: module parameter references must be preserved
// as `W` rather than being evaluated to a concrete number.
module veryl_testcase_Module88b #(
    parameter int unsigned W = 8
);
    logic [W-1:0] _pa;
    always_comb _pa = 0;

    // Variable reference: width expression `W` is kept intact.
    logic [W-1:0] _pb; always_comb _pb = _pa;

    // `var` without type annotation: type is inferred from the first
    // subsequent `assign` statement.
    logic [W-1:0] _pc;
    always_comb _pc = _pa;

    // Type of `_pd` is inferred from its first assignment inside
    // always_comb.
    logic [W-1:0] _pd;
    always_comb begin
        _pd = _pa;
    end

    // Multi-stage inference: _pe is inferred from _pc which itself
    // was inferred earlier in this module.
    logic [W-1:0] _pe;
    always_comb _pe = _pc;
endmodule

module veryl_testcase_Module88;
    logic [8-1:0] _a; always_comb _a = 0;

    // Variable reference: type copied from the referenced variable.
    logic [8-1:0] _b; always_comb _b = _a;

    // Sized literal: type implied by the literal itself.
    bit [8-1:0] _c; always_comb _c = 8'd255;

    // Parenthesized expression recurses.
    logic [8-1:0] _d; always_comb _d = (_a);

    // Const declarations also support inference.
    localparam bit [16-1:0] _E = 16'd100;

    always_comb begin
        // Let statement inside a block.
        logic [8-1:0] _e;
        _e = _a;
    end
endmodule
//# sourceMappingURL=../map/88_type_inference.sv.map
