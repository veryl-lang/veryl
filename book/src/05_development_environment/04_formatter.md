# Formatter

Source code can be formatted by `veryl fmt` command.
Alternatively, language server support formatting through `textDocument/formatting` request.

The available configurations are below.
These can be specified in `[format]` section of `Veryl.toml`.

```toml
[format]
indent_width = 4
```

| Configuration | Value   | Description           |
|---------------|---------|-----------------------|
| indent_width  | integer | indent width by space |
