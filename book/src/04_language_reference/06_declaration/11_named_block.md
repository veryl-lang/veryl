# Named Block

Label can be added to `{}` block.
The named block has an individual namespace.

```veryl,playground
module ModuleA {
    :labelA {
        var _a: logic<10>;
    }

    :labelB {
        var _a: logic<10>;
    }
}
```
