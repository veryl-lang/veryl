# Instantiation

`inst` keyword represents instantiation of modula and interface.
The name of instance is placed after `inst` keyword,
and the type of instance is placed after `:`.
Parameter override is `#()`, and port connection is `()`.

```veryl,playground
module ModuleA #(
    parameter paramA: u32 = 1,
) {
    var a: logic<10>;
    var b: logic<10>;

    inst instB: ModuleB #(
        paramA    , // Parameter assignment by name
        paramB: 10,
    ) (
        a    , // Port connection by name
        bb: b,
    );
}
```
