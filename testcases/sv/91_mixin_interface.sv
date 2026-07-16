// interface inheritance

package veryl_testcase_Package91A;
    typedef logic [8-1:0] T;
endpackage

interface veryl_testcase_Interface91A;
    import veryl_testcase_Package91A::*;

    T a;

    modport mp_a (
        input a
    );
endinterface

package veryl_testcase___Package91B__16;
    typedef logic [16-1:0] T;
endpackage

interface veryl_testcase___Interface91B__16;
    import veryl_testcase___Package91B__16::*;

    T b;

    modport mp_b (
        input b
    );
endinterface

package veryl_testcase___Package91C__32;
    typedef logic [32-1:0] T;
endpackage

interface veryl_testcase___Interface91C__16__32;
    import veryl_testcase___Package91C__32::*;


    veryl_testcase_Package91A::T a;

    modport mp_a (
        input a
    );


    veryl_testcase___Package91B__16::T b;

    modport mp_b (
        input b
    );

    T c;

    modport mp_abc (
        input c,
        input a,
        input b
    );
endinterface

module veryl_testcase___Module91A__16__32 (
    veryl_testcase___Interface91C__16__32.mp_abc abc
);
endmodule

module veryl_testcase_Module91B;
    veryl_testcase___Interface91C__16__32 abc ();

    always_comb begin
        abc.a = 0;
        abc.b = 0;
        abc.c = 0;
    end

    veryl_testcase___Module91A__16__32 d (
        .abc (abc)
    );
endmodule
//# sourceMappingURL=../map/91_mixin_interface.sv.map
