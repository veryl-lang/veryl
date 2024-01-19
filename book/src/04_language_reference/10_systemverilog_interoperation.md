# SystemVerilog Interoperation

If you want to access to items of SystemVerilog, `$sv` namespace can be used.
For example, "ModuleA" in SystemVerilog source code can be accessed by `$sv::ModuleA`.
Veryl don't check the existence of the items.

```veryl,playground
module ModuleA {
    var _a: logic = $sv::PackageA::ParamA;

    inst b: $sv::ModuleB ();
    inst c: $sv::InterfaceC ();
}
```
