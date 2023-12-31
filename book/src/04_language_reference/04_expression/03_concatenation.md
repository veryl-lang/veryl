# Concatenation

`{}` represents bit concatenation.
In `{}`, `repeat` keyword can be used to repeat specified operand.

```veryl,playground
module ModuleA {
    var a: logic<10>;
    var b: logic<10>;
    var _c: logic = {a[9:0], b[4:3]};
    var _d: logic = {a[9:0] repeat 10, b repeat 4};
}
```
