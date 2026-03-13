interface veryl_testcase_Interface75;
    logic a;
    logic b;
    logic c;
    logic d;

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
        input  a    ,
        output c    ,
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
        output d    ,
        import get_a,
        import get_b,
        import set_c,
        import set_d
    );

    modport slave_ac (
        output a,
        input  c
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
    );

    modport all_input (
        input a,
        input b,
        input c,
        input d
    );

    modport all_output (
        input a,
        input b,
        input c,
        input d
    );

    modport partial_converse (
        input  a,
        output b,
        input  c,
        input  d
    );

    modport partial_input (
        output c,
        input  a,
        input  b,
        input  d
    );

    modport partial_same (
        output a    ,
        input  b    ,
        output c    ,
        output d    ,
        import get_a,
        import get_b,
        import set_c,
        import set_d
    );
endinterface
//# sourceMappingURL=../map/75_modport_default.sv.map
