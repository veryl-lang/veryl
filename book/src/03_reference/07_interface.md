# Interface

```veryl,playground
interface simple_bus {
    var req  : logic;
    var gnt  : logic;
    var addr : logic [8];
    var data : logic [8];
    var mode : logic [2];
    var start: logic;
    var rdy  : logic;

    modport master {
        req  : output,
        addr : output,
        mode : output,
        start: output,
        gnt  : input ,
        rdy  : input ,
        data : ref   ,
    }
}
```
