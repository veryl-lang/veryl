# Function Call

Function can be call by `function_name(argument)`.
System function of SystemVerilog like `$clog2` can be used too.

```veryl,playground
module ModuleA {
    var _a: logic = PackageA::FunctionA(1, 1);
    var _b: logic = $clog2(1, 1);
}
```
