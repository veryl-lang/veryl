# Initial / Final

Statements in `initial` are executed at the beginning of simulation,
`final` is the end.
Both will be ignored logical synthesis, and can be used as debug or assertion.

```veryl,playground
module ModuleA {
    initial {
        $display("initial");
    }

    final {
        $display("final");
    }
}
```
