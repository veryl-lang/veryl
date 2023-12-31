# Source Code Structure

Veryl's source code is composed by some `module`, `interface` and `package`.

```veryl,playground
module ModuleA {
}

module ModuleB {
}

interface InterfaceA {
}

package PackageA {
}
```

The name of `module`, `interface` and `package` in the transpiled code will added project name as prefix.
In the sample code, `project_` will be added.
It is to avoid name conflict between projects.
