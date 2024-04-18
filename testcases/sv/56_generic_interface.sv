module veryl_testcase_Module56;
    veryl_testcase___Interface56A__Package56A u0 ();
    veryl_testcase___Interface56A__Package56B u1 ();
endmodule

interface veryl_testcase___Interface56A__Package56A;
    logic [veryl_testcase_Package56A::X-1:0] _a;
endinterface
interface veryl_testcase___Interface56A__Package56B;
    logic [veryl_testcase_Package56B::X-1:0] _a;
endinterface

package veryl_testcase_Package56A;
    localparam int unsigned X = 1;
endpackage

package veryl_testcase_Package56B;
    localparam int unsigned X = 2;
endpackage
