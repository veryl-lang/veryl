# Package

Package is one of top level components in source code.
Package can organize some declarations like parameter and function.

To access an item in a package, `::` symbol can be used like `PackageA::ParamA`.

```veryl,playground
package PackageA {
    localparam ParamA: u32 = 0;
}
```
