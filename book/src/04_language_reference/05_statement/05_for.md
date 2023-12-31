# For

`for` statement represent repetition.
Loop variable is placed before `in` keyword,
and [range](../04_expression/07_range.md) is placed after it.

```veryl,playground
module ModuleA {
    var a: logic<10>;

    always_comb {
        for i: u32 in 0..10 {
            a += i;
        }
    }
}
```
