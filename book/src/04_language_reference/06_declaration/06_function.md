# Function

Function can be declared by `function` keyword.
Arguments are placed in `()` and return type is placed after `->`.

If function doesn't have a return value, `->` can be omitted.

```veryl,playground
module ModuleA {
    var a: logic<10>;
    var b: logic<10>;

    function FunctionA (
        a: input logic<10>
    ) -> logic<10> {
        return a + 1;
    }

    function FunctionB (
        a: input logic<10>
    ) {
    }

    assign b = FunctionA(a);

    initial {
        FunctionB(a);
    }
}
```
