# Dependencies

If you want to add other Veryl projects to dependencies of your project, you can add them to `[dependencies]` section in `Veryl.toml`.
The left hand side of entry is path to the dependency, and the right hand side is version.

```toml
[dependencies]
"https://github.com/dalance/veryl_sample" = "0.1.0"
```

By default, the namespace of the dependency is the same as the project name of the dependency.
If you want to specify namespace, you can use `name` field.

```toml
[dependencies]
"https://github.com/dalance/veryl_sample" = {version = "0.1.0", name = "veryl_sample_alt"}
```

If you want to use many versions of the same dependency path, you can specify each name.

```toml
[dependencies]
"https://github.com/dalance/veryl_sample" = [
    {version = "0.1.0", name = "veryl_sample1"},
    {version = "0.2.0", name = "veryl_sample2"},
]
```

## Usage of dependency

After adding dependencies to `Veryl.toml`, you can use `moudle`, `interface` and `package` in the dependencies.
The following example uses `delay` module in the `veryl_sample` dependency.

```veryl,playground
module ModuleA (
    i_clk  : input  logic,
    i_rst_n: input  logic,
    i_d    : input  logic,
    o_d    : output logic,
) {
    inst u_delay: veryl_sample::delay (
        i_clk  ,
        i_rst_n,
        i_d    ,
        o_d    ,
    );
}
```

> Note: The result of play button in the above code is not exact because it doesn't use dependency resolution.
> Actually the module name becomes `veryl_samlle_delay`

## Version Requirement

The `version` field of `[dependencies]` section shows version requirement.
For example, `version = "0.1.0"` means the latest version which has compatibility with `0.1.0`.
The compatibility is judged by [Semantic Versioning](https://semver.org/).
A version is constructed from the following three parts.

* `MAJOR` version when you make incompatible API changes
* `MINOR` version when you add functionality in a backwards compatible manner
* `PATCH` version when you make backwards compatible bug fixes

If `MAJOR` version is `0`, `MINOR` is interpreted as incompatible changes.

If there are `0.1.0` and `0.1.1` and `0.2.0`, `0.1.1` will be selected.
This is because

* `0.1.0` is compatible with `0.1.0`.
* `0.1.1` is compatible with `0.1.0`.
* `0.2.0` is not compatible with `0.1.0`.
* `0.1.1` is the latest in the compatible versions.

The `version` field allows other version requirement reprensentation like `=0.1.0`.
Please see version requirement of Rust for detailed information: [Specifying Dependencies](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html#specifying-dependencies-from-cratesio).
