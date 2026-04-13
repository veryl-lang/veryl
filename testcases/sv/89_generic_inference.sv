module veryl_testcase_Module89;
    function automatic logic [8-1:0] __FuncId__8(
        input var logic [8-1:0] x
    ) ;
        return x;
    endfunction
    function automatic logic [16-1:0] __FuncId__16(
        input var logic [16-1:0] x
    ) ;
        return x;
    endfunction

    function automatic logic [(8 + 1)-1:0] __FuncWide__8(
        input var logic [8-1:0] x
    ) ;
        return {1'b0, x};
    endfunction

    logic [8-1:0]  _a; always_comb _a = 0;
    logic [16-1:0] _b; always_comb _b = 0;

    // Generic argument inferred from the argument's declared width.
    logic [8-1:0]  _r1; always_comb _r1 = __FuncId__8(_a);
    logic [16-1:0] _r2; always_comb _r2 = __FuncId__16(_b);

    // `T + 1` pattern inference: argument width 8 resolves T = 8.
    logic [9-1:0] _rw; always_comb _rw = __FuncWide__8(_a);
endmodule
//# sourceMappingURL=../map/89_generic_inference.sv.map
