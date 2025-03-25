```systemverilog
module Top (
    a: input logic<32>,
    b: input logic<32>,
    c: output logic<32>,
){
    assign c = a + b;
}
```

Simulator should execute the following Rust code using the above Veryl code.

```rust
let mut sim = Simulator::new("Top");
sim.set("a", 10);
sim.set("b", 20);
sim.step();
assert_eq!(sim.get("c"), 30);
```

I placed the above test at `./src/tests.rs`.


I think simulator should have the following information:

* Variable table to keep the current value (e.g. `a, b, c` in the above example)
* Statement table for execution (e.g. assign statement in the above example)

The execution step of simulator becomes:

* At calling `new`
  * Set all variables in variable table unevaluated
* At calling `set`
  * Set value to variable, and clear unevaluated flag
* At calling `step`
  * Get unevaluated variable from variable table (e.g. `c` is unevaluated)
  * Evaluate statement (assign statement in this case) corresponding to the variable
  * Evaluate variables recursively if unevaluated variable appears in evaluating statement
  * Stop when there is not unevaluated variable.

Such way for execution is necessary because RTL is not executed from the top of source code to the bottom, each statement is executed parallelly.


To construct variable table and statement table from Veryl source code, symbol table can be used.
Symbol table has symbol information corresponding to identifier.
The definition of symbol struct is here:

https://docs.rs/veryl-analyzer/latest/veryl_analyzer/symbol/struct.Symbol.html

The kind of symbol is `SymbolKind`:

https://docs.rs/veryl-analyzer/latest/veryl_analyzer/symbol/enum.SymbolKind.html

And structs in each variant have kind specific information.
For example, `ModuleProperty` has `definition` member, which can access whole syntax tree of module definition, and assign statement can be extracted from it.
In future, it may be better that Veryl compiler extracts information which is required by simulator during analysis phase.
Variable can be identified that `SymbolKind` is `Port` or `Variable`. You can know whether it is in a top module by checking namespace.

In Veryl compiler, resources, which needs heap like `String`, are managed throuh ID (usize) because it is troublesome for copy and borrow.
The actual resources are placed at `HashMap` in thread local storage. So if you want to refer the actual value, it should be refered from the table.
There are some example in `./src/tests.rs`.

About evaluating expression, the implementation of `Evaluator` may be helpful.
In future, it may be better to merge evaluator for compiler and for simulator, but at the moment, it can be implemented for similator specific to explore implementation.

https://docs.rs/veryl-analyzer/latest/veryl_analyzer/evaluator/struct.Evaluator.html

I recommend to start easiest case like the first example, and step by step like below:

* always_comb statement
* introduce clock (always_ff statement)
* module hierarchy
* calling funciton support

Additionally, variable size becomes too large (e.g. 256bit, 1024bit, and so on) in RTL, so I recommend to migrate something like big integer crate even if you use `usize` at first.

Syntax definition is below.
Rust code is generated from the definition, so the node name in the definition is the same as struct name in Rust.
If you want to know how the syntax element which you want to get is constructed, it may be helpful.

https://github.com/veryl-lang/veryl/blob/master/crates/parser/veryl.par
