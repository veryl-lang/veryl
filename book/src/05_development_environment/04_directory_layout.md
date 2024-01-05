# Directory Layout

Veryl supports arbitrary directory layout.
This is because the optimal directory layout for an independent project and an integrated project within other projects is different.

In this section, we suggest some directory layout patterns.

## Single source directory

This pattern contains all sources in `src` directory.
In `src`, you can configure arbitrary sub directories.

```
$ tree
.
|-- src
|   |-- module_a.veryl
|   `-- module_b
|       |-- module_b.veryl
|       `-- module_c.veryl
`-- Veryl.toml

2 directories, 4 files
```

Veryl gathers all `*.veryl` files and generates codes at the same directory as the source by default.
You can show the behaviour explicitly by the following configuration.

```toml
[build]
target = "source"
```

After `veryl build`, the directory structure will become below:

```
$ tree
.
|-- dependencies
|-- prj.f
|-- src
|   |-- module_a.sv
|   |-- module_a.veryl
|   `-- module_b
|       |-- module_b.sv
|       |-- module_b.veryl
|       |-- module_c.sv
|       `-- module_c.veryl
`-- Veryl.toml

3 directories, 8 files
```

## Single source and target directory

If you want to place the generated codes into a directory, you can use `target` configure in `[build]` section of `Veryl.toml`.

```toml
[build]
target = {type = "directory", path = "target"}
```

The directory layout of this configure will become below:

```
$ tree
.
|-- dependencies
|-- prj.f
|-- src
|   |-- module_a.veryl
|   `-- module_b
|       |-- module_b.veryl
|       `-- module_c.veryl
|-- target
|   |-- module_a.sv
|   |-- module_b.sv
|   `-- module_c.sv
`-- Veryl.toml

4 directories, 8 files
```

## Multi source directory

If you want to add a veryl project to the existing SystemVerilog project, you can choose the following structure.

```
$ tree
.
|-- dependencies
|-- module_a
|   |-- module_a.sv
|   `-- module_a.veryl
|-- module_b
|   |-- module_b.sv
|   |-- module_b.veryl
|   |-- module_c.sv
|   `-- module_c.veryl
|-- prj.f
|-- sv_module_x
|   `-- sv_module_x.sv
|-- sv_module_y
|   `-- sv_module_y.sv
`-- Veryl.toml

5 directories, 10 files
```

The generated `prj.f` lists all generated files. So you can use it along with the existing SystemVerilog filelists.

## About `.gitignore`

Veryl doesn't provide the default `.gitignore`.
This is because which files should be ignored is different by each projects.

The candidates of `.gitignore` is below:

* `dependencies/`
* `target/`
* `*.sv`
* `*.f`
