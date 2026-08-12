interface veryl_testcase_Interface75;
    logic a;
    logic b;
    logic c;
    logic d;

    `ifdef TRACE
    logic f;
    `endif

    function automatic logic Func75() ;
        logic e;
        e = 0;
        return e;
    endfunction

    function automatic logic get_a() ;
        return a;
    endfunction

    function automatic logic get_b() ;
        return b;
    endfunction

    function automatic void set_c(
        input var logic x
    ) ;
        c = x;
    endfunction

    function automatic void set_d(
        input var logic x
    ) ;
        d = x;
    endfunction

    modport master_ac (
        input  a,
        output c,
        `ifdef TRACE
        input  f    ,
        `endif
        import get_a,
        import set_c
    );

    modport master_bd (
        input  b    ,
        output d    ,
        import get_b,
        import set_d
    );

    modport master (
        input  a    ,
        input  b    ,
        output c    ,
        output d    
        `ifdef TRACE
        , input  f    ,
        `endif
        import get_a,
        import get_b,
        import set_c,
        import set_d
    );

    modport slave_ac (
        output a,
        input  c
        `ifdef TRACE
        , output f
        `endif
    );

    modport slave_db (
        output b,
        input  d
    );

    modport slave (
        output a,
        output b,
        input  c,
        input  d
        `ifdef TRACE
        , output f
        `endif
    );

    modport all_input (
        input a,
        input b,
        input c,
        input d
        `ifdef TRACE
        , input f
        `endif
    );

    modport all_output (
        input a,
        input b,
        input c,
        input d
        `ifdef TRACE
        , input f
        `endif
    );

    modport partial_converse (
        input  a,
        output b,
        input  c,
        input  d
        `ifdef TRACE
        , output f
        `endif
    );

    modport partial_input (
        output c,
        input  a,
        input  b,
        input  d
        `ifdef TRACE
        , input  f
        `endif
    );

    modport partial_same (
        output a    ,
        input  b    ,
        output c    ,
        output d    
        `ifdef TRACE
        , input  f    ,
        `endif
        import get_a,
        import get_b,
        import set_c,
        import set_d
    );
endinterface
//# sourceMappingURL=../map/75_modport_default.sv.map
