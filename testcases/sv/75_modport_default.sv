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

    modport master_ac (
        input  a,
        output c
    );

    modport master_bd (
        input  b,
        output d
    );

    modport master (
        input  a,
        input  b,
        output c,
        output d
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
        output a,
        input  b,
        output c,
        output d
    );
endinterface
//# sourceMappingURL=../map/75_modport_default.sv.map
