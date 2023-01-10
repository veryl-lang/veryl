import PackageA::A;
import PackageA::*;

module Module19 ;
    import PackageA::A;
    import PackageA::*;
endmodule

interface Interface19 ;
    import PackageA::A;
    import PackageA::*;
endinterface

package Package19;
    import PackageA::A;
    import PackageA::*;
    export PackageA::A;
    export *::*;
endpackage
