# Bit Select

`[]` is bit select operator.
If an expression is specified to `[]`, single bit is selected.
Bit range selection can be specified by `[expression:expression]`.

```veryl,playground
module ModuleA {
    var a: logic<10>;
    var b: logic<10>;
    var c: logic<10>;

    assign b = a[3];
    assign c = a[4:0];
}
```
