interface veryl_testcase_Interface75;
    `ifdef TRACE
    logic f;
    `else
    logic g;
    `endif

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
        input  a,
        output c,
        `ifdef TRACE
        input f,
        `else
        input  g    ,
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
        `ifdef TRACE
        input  f    ,
        `endif
        `ifndef TRACE
        input  g    ,
        `endif
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
        `ifdef TRACE
        output f,
        `endif
        `ifndef TRACE
        output g,
        `endif
        output a,
        input  c
    );

    modport slave_db (
        output b,
        input  d
    );

    modport slave (
        `ifdef TRACE
        output f,
        `endif
        `ifndef TRACE
        output g,
        `endif
        output a,
        output b,
        input  c,
        input  d
    );

    modport all_input (
        `ifdef TRACE
        input f,
        `endif
        `ifndef TRACE
        input g,
        `endif
        input a,
        input b,
        input c,
        input d
    );

    modport all_output (
        `ifdef TRACE
        input f,
        `endif
        `ifndef TRACE
        input g,
        `endif
        input a,
        input b,
        input c,
        input d
    );

    modport partial_converse (
        input  a,
        `ifdef TRACE
        output f,
        `endif
        `ifndef TRACE
        output g,
        `endif
        output b,
        input  c,
        input  d
    );

    modport partial_input (
        output c,
        `ifdef TRACE
        input  f,
        `endif
        `ifndef TRACE
        input  g,
        `endif
        input  a,
        input  b,
        input  d
    );

    modport partial_same (
        output a    ,
        `ifdef TRACE
        input  f    ,
        `endif
        `ifndef TRACE
        input  g    ,
        `endif
        input  b    ,
        output c    ,
        output d    ,
        import get_a,
        import get_b,
        import set_c,
        import set_d
    );
endinterface

interface veryl_testcase_Interface75MemberDefine;
    logic b;
    logic a;

    modport master (
        `ifdef TRACE
        input b,
        `endif
        input a
    );

    modport slave (
        `ifdef TRACE
        output b,
        `endif
        output a
    );

    modport relay (
        `ifdef TRACE
        output b,
        `endif
        output a
    );
endinterface
//# sourceMappingURL=../map/75_modport_default.sv.map
