# Hello, World!

## Create Project

At first, a new Veryl project can be created by:

```
veryl new hello
```

After the command, the following directory and file will be created.

```
$ veryl new hello
[INFO ]      Created "hello" project
$ cd hello
$ tree
.
`-- Veryl.toml

0 directories, 1 file
```

`Veryl.toml` is the project configuration.

```toml
[project]
name = "hello"
version = "0.1.0"
```

The description of all configuration is [here]().

## Write Code

You can add source codes at an arbitrary position in the project directory.
The extension of Veryl's source codes is `.vl`.

For example, put the following code to `src/hello.vl`.

```veryl,playground
module ModuleA {
}
```

```
$ tree
.
|-- src
|   `-- hello.vl
`-- Veryl.toml

1 directory, 2 files
```

## Build Code

You can generate a SystemVerilog code by `veryl build`.

```
$ veryl build
[INFO ]   Processing file ([path to hello]/src/hello.vl)
[INFO ]       Output filelist ([path to hello]/hello.f)
$ tree
.
|-- dependencies
|-- hello.f
|-- src
|   |-- hello.sv
|   `-- hello.vl
`-- Veryl.toml

2 directories, 4 files
```

By default, SystemVerilog code will be generated at the same directory as Veryl code.
The generated code is `src/hello.sv`.

```verilog
module hello_ModuleA;

endmodule
```

Additionally, `hello.f` which is the filelist of generated codes will be generated.
You can use it for SystemVerilog compiler.
The following example is to use [Verilator](https://www.veripool.org/verilator/).

```
$ verilator --cc -f hello.f
```
