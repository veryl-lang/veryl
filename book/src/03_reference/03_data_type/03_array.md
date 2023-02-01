# Array

Array can be defined by appending `[]` to any data type.
The length of array can be specified by the value in `[]`.

```veryl,playground
module ModuleA {
    var a: logic     [20];
    var b: logic<10> [20];
    var c: u32       [20];
    var d: StructA   [20];
    var e: EnumA     [20];

    assign a[0] = 0;
    assign b[0] = 0;
    assign c[0] = 0;
    assign d[0] = 0;
    assign e[0] = 0;
}
```

Multi-dimentional array can be defined by `[X, Y, Z,,,]`.

```veryl,playground
module ModuleA {
    var a: logic     [10, 20, 30];
    var b: logic<10> [10, 20, 30];
    var c: u32       [10, 20, 30];
    var d: StructA   [10, 20, 30];
    var e: EnumA     [10, 20, 30];

    assign a[0][0][0] = 0;
    assign b[0][0][0] = 0;
    assign c[0][0][0] = 0;
    assign d[0][0][0] = 0;
    assign e[0][0][0] = 0;
}
```
