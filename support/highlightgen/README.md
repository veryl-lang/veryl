# highlightgen

highlightgen generates syntax highlight definitions for several highlighters from `crates/parser/veryl.par`.

## Usage

Some syntax highlight definitions is under git submodule.
So fetching submodules is necessary.

```
$ git submodule update --init --recursive
```

To check the current definitions is consistent with `veryl.par`,
the following command can be used.

```
$ cargo run --bin highlightgen -- check
```

To generate new definitions from `veryl.par`,
the following command can be used.

```
$ cargo run --bin highlightgen -- build
```

## Keyword Category

There are the following keyword categories.

* `Conditional`
* `Direction`
* `Literal`
* `Repeat`
* `Statement`
* `Structure`
* `Type`

In `veryl.par`, each keyword line has a comment to specify category at the end of line like below:

```
// Keyword: Conditional
```
