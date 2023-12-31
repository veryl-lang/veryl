# Case

`case` can be used as statement.
The right-hand of arm is statement.

```veryl,playground
module ModuleA {
    var a: logic<10>;
    var b: logic<10>;

    always_comb {
        case a {
            0: b = 1;
            1: b = 2;
            2: {
                b = 3;
                b = 3;
                b = 3;
            }
            default: b = 4;
        }
    }
}
```
