# Project Configuration

* [`[project]`](01_project_configuration.md#the-project-section) --- Defines a project.
  * [`name`](01_project_configuration.md#the-name-field) --- The name of the project.
  * [`version`](01_project_configuration.md#the-version-field) --- The version of the project.
  * [`authors`](01_project_configuration.md#the-authors-field) --- The authors of the project.
  * [`description`](01_project_configuration.md#the-description-field) --- A description of the project.
  * [`license`](01_project_configuration.md#the-license-field) --- The project license.
  * [`repository`](01_project_configuration.md#the-repository-field) --- URL of the project source repository.
* [`[build]`](01_project_configuration.md#the-build-section) --- Build settings.
  * [`clock_type`](01_project_configuration.md#the-clock_type-field) --- The type of clock.
  * [`reset_type`](01_project_configuration.md#the-reset_type-field) --- The type of reset.
  * [`filelist_type`](01_project_configuration.md#the-filelist_type-field) --- The type of filelist.
  * [`target`](01_project_configuration.md#the-target-field) --- The way of output.
  * [`implicit_parameter_types`](01_project_configuration.md#the-implicit_parameter_types-field) --- Whether implicit parameter type is enabled.
* [`[format]`](01_project_configuration.md#the-format-section) --- Format settings.
* [`[lint]`](01_project_configuration.md#the-lint-section) --- Lint settings.
* [`[publish]`](01_project_configuration.md#the-publish-section) --- Publish settings.
* [`[dependencies]`](01_project_configuration.md#the-dependencies-section) --- Library dependencies.

## The `[project]` section {#the-project-section}

The first section of `Veryl.toml` is `[project]`.
The mandatory fields are `name` and `version`.

### The `name` field {#the-name-field}

The project name is used as prefix in the generated codes.
So the name must start with alphabet or `_`, and use only alphanumeric charaters or `_`.

### The `version` field {#the-version-field}

The project version should follow [Semantic Versioning](https://semver.org/).
The version is constructed by the following three numbers.

* Major -- increment at incompatible changes
* Minor -- increment at adding features with backward compatibility
* Patch -- increment at bug fixes with backward compatibility

```toml
[project]
version = "0.1.0"
```

### The `authors` field {#the-authors-field}

The optional `authors` field lists in an array the people or organizations that are considered the "authors" of the project.
The format of each string in the list is free. Name only, e-mail address only, and name with e-mail address included within angled brackets are commonly used.

```toml
[project]
authors = ["Fnu Lnu", "anonymous@example.com", "Fnu Lnu <anonymous@example.com>"]
```

### The `description` field {#the-description-field}

The `description` is a short blurb about the project. This should be plane text (not Markdown).

### The `license` field {#the-license-field}

The `license` field contains the name of license that the project is released under.
The string should be follow [SPDX 2.3 license expression](https://spdx.github.io/spdx-spec/v2.3/SPDX-license-expressions).

```toml
[project]
license = "MIT OR Apache-2.0"
```

### The `repository` field {#the-repository-field}

The `repository` field should be a URL to the source repository for the project.

```toml
[project]
repository = "https://github.com/dalance/veryl"
```

## The `[build]` section {#the-build-section}

The `[build]` section contains the configurations of code generation.

### The `clock_type` field {#the-clock_type-field}

The `clock_type` field specifies which clock edge is used to drive flip-flop.
The available types are below:

* `posedge` -- positive edge
* `negedge` -- negetive edge

### The `reset_type` field {#the-reset_type-field}

The `reset_type` field specifies reset polarity and synchronisity.
The available types are below:

* `async_low` -- asynchronous and active low
* `async_high` -- asynchronous and active high
* `sync_low` -- synchronous and active low
* `sync_high` -- synchronous and active high

### The `filelist_type` field {#the-filelist_type-field}

The `filelist_type` field specifies filelist format.
The available types are below:

* `absolute` -- plane text filelist including absolute file paths
* `relative` -- plane text filelist including relative file paths
* `flgen` -- [flgen](https://github.com/pezy-computing/flgen) filelist

### The `target` field {#the-target-field}

The `target` field specifies where the generated codes will be placed at.
The available types are below:

* `source` -- as the same directory as the source code
* `directory` -- specified directory

If you want to use `directory`, you can specify the target directory by `path` key.

```toml
[build]
target = {type = "directory", path = "[dst dir]"}
```

### The `implicit_parameter_types` field {#the-implicit_parameter_types-field}

The `implicit_parameter_types` field lists the types which will be elided in `parameter` declaration of the generated codes.
This is because some EDA tools don't support `parameter` declaration with specific types (ex.`string`).
If you want to elide `string`, you can specify like below:

```toml
[build]
implicit_parameter_types = ["string"]
```

## The `[format]` section {#the-format-section}

The `[format]` section contains the configurations of code formatter.
Available configurations is [here](./05_formatter.md).

## The `[lint]` section {#the-lint-section}

The `[lint]` section contains the configurations of linter.
Available configurations is [here](./06_linter.md).

## The `[publish]` section {#the-publish-section}

The `[publish]` section contains the configurations of publishing.
Available configurations is [here](./03_publish_project.md).

## The `[dependencies]` section {#the-dependencies-section}

The `[dependencies]` section contains library dependencies.
Available configurations is [here](./02_dependencies.md).
