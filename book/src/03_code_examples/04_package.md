# Package

```veryl,playground,editable
// package definition
package PackageA {
    localparam ParamA: u32 = 1;
    localparam ParamB: u32 = 1;

    function FuncA (
        a: input  logic<ParamA>,
    ) -> logic<ParamA> {
        return a + 1;
    }
}

module ModuleA {
    var a : logic<10> = PackageA::ParamA;
    var _b: logic<10> = PackageA::FuncA(a);
}
```
