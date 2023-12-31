# Assignment

Assignment statement is `variable = expression;`.
Unlike SystemVerilog, assignment operator is `=` in both `always_comb` and `always_ff`.
There are other assignment operators:

* `+=`: addition assignment
* `-=`: subtraction assignment
* `*=`: multiplication assignment
* `/=`: division assignment
* `%=`: remainder assignment
* `&=`: bitwise AND assignment
* `|=`: bitwise OR assignment
* `^=`: bitwise XOR assignment
* `<<=`: logical left shift assignment
* `>>=`: logical right shift assignment
* `<<<=`: arithmetic left shift assignment
* `>>>=`: arithmetic right shift assignment

```veryl,playground
module ModuleA (
    i_clk: input logic,
) {
    var a: logic<10>;
    var b: logic<10>;
    var c: logic<10>;
    var d: logic<10>;
    var e: logic<10>;

    always_comb {
        b =  a + 1;
        c += a + 1;
    }

    always_ff (i_clk) {
        d =  a + 1;
        e -= a + 1;
    }
}
```
