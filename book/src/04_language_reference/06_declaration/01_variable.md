# Variable

Variable declaration is started by `var` keyword.
After `var`, variable name, `:`, and the type of the variable are followed.

If there are unused variables, warning will be occured.
Variable name starting with `_` means unused variable, and suppresses the warning.

Variable declaration with assignment can be used too.

```veryl,playground
module ModuleA {
    var _a: logic        ;
    var _b: logic<10>    ;
    var _c: logic<10, 10>;
    var _d: u32          ;
    var _e: logic        = 1;
}
```
