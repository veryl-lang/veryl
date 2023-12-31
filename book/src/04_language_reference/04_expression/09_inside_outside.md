# Inside / Outside

`inside` check the specified expression is inside conditions which are specified in `{}`.
Condition can be single expression or [range](./07_range.md).
If the expression matches any condition, `inside` will return `1`, otherwise `0`.
`outside` is vice versa.

```veryl,playground
module ModuleA {
    var a: logic;
    var b: logic;

    assign a = inside 1 + 2 / 3 {0, 0..10, 1..=10};
    assign b = outside 1 * 2 - 1 {0, 0..10, 1..=10};
}
```
