# Module

```veryl,playground
module mux2to1 (
    a  : input  logic,
    b  : input  logic,
    sel: input  logic,
    y  : output logic,
) {
    always_comb {
        if sel {
            y = a;
        } else {
            y = b;
        }
    }
}
```
