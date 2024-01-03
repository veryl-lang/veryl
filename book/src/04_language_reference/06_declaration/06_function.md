# Function

Function can be declared by `function` keyword.
Arguments are placed in `()` and return type is placed after `->`.

```veryl,playground
module ModuleA {
    var a: logic<10>;
    var b: logic<10>;

    function FunctionA (
        a: input logic<10>
    ) -> logic<10> {
        return a + 1;
    }

    assign b = FunctionA(a);
}
```
