# Linter

Lint check is executed at `veryl check` or `veryl build`.
Alternatively, language server checks lint in real time.

The available configurations are below.
These can be specified in `[lint]` section of `Veryl.toml`.

```toml
[lint.naming]
case_enum = "snake"
```

## The `[lint.naming]` section

This section contains configurations of naming conventions.

| Configuration             | Value                | Description                                     |
|---------------------------|----------------------|-------------------------------------------------|
| case_enum                 | case type[^casetype] | case style of `enum`                            |
| case_function             | case type[^casetype] | case style of `function`                        |
| case_instance             | case type[^casetype] | case style of instance                          |
| case_interface            | case type[^casetype] | case style of `interface`                       |
| case_modport              | case type[^casetype] | case style of `modport`                         |
| case_module               | case type[^casetype] | case style of `module`                          |
| case_package              | case type[^casetype] | case style of `package`                         |
| case_parameter            | case type[^casetype] | case style of `parameter`                       |
| case_port_inout           | case type[^casetype] | case style of `inout` port                      |
| case_port_input           | case type[^casetype] | case style of `input` port                      |
| case_port_modport         | case type[^casetype] | case style of `modport` port                    |
| case_port_output          | case type[^casetype] | case style of `output` port                     |
| case_reg                  | case type[^casetype] | case style of register type variable[^reg]      |
| case_struct               | case type[^casetype] | case style of `struct`                          |
| case_wire                 | case type[^casetype] | case style of wire type variable[^wire]         |
| prefix_enum               | string               | prefix of `enum`                                |
| prefix_function           | string               | prefix of `function`                            |
| prefix_instance           | string               | prefix of instance                              |
| prefix_interface          | string               | prefix of `interface`                           |
| prefix_modport            | string               | prefix of `modport`                             |
| prefix_module             | string               | prefix of `module`                              |
| prefix_package            | string               | prefix of `package`                             |
| prefix_parameter          | string               | prefix of `parameter`                           |
| prefix_port_inout         | string               | prefix of `inout` port                          |
| prefix_port_input         | string               | prefix of `input` port                          |
| prefix_port_modport       | string               | prefix of `modport` port                        |
| prefix_port_output        | string               | prefix of `output` port                         |
| prefix_reg                | string               | prefix of register type variable[^reg]          |
| prefix_struct             | string               | prefix of `struct`                              |
| prefix_wire               | string               | prefix of wire type variable[^wire]             |
| re_forbidden_enum         | regex[^regex]        | regex forbidden of `enum`                       |
| re_forbidden_function     | regex[^regex]        | regex forbidden of `function`                   |
| re_forbidden_instance     | regex[^regex]        | regex forbidden of instance                     |
| re_forbidden_interface    | regex[^regex]        | regex forbidden of `interface`                  |
| re_forbidden_modport      | regex[^regex]        | regex forbidden of `modport`                    |
| re_forbidden_module       | regex[^regex]        | regex forbidden of `module`                     |
| re_forbidden_package      | regex[^regex]        | regex forbidden of `package`                    |
| re_forbidden_parameter    | regex[^regex]        | regex forbidden of `parameter`                  |
| re_forbidden_port_inout   | regex[^regex]        | regex forbidden of `inout` port                 |
| re_forbidden_port_input   | regex[^regex]        | regex forbidden of `input` port                 |
| re_forbidden_port_modport | regex[^regex]        | regex forbidden of `modport` port               |
| re_forbidden_port_output  | regex[^regex]        | regex forbidden of `output` port                |
| re_forbidden_reg          | regex[^regex]        | regex forbidden of register type variable[^reg] |
| re_forbidden_struct       | regex[^regex]        | regex forbidden of `struct`                     |
| re_forbidden_wire         | regex[^regex]        | regex forbidden of wire type variable[^wire]    |
| re_required_enum          | regex[^regex]        | regex required of `enum`                        |
| re_required_function      | regex[^regex]        | regex required of `function`                    |
| re_required_instance      | regex[^regex]        | regex required of instance                      |
| re_required_interface     | regex[^regex]        | regex required of `interface`                   |
| re_required_modport       | regex[^regex]        | regex required of `modport`                     |
| re_required_module        | regex[^regex]        | regex required of `module`                      |
| re_required_package       | regex[^regex]        | regex required of `package`                     |
| re_required_parameter     | regex[^regex]        | regex required of `parameter`                   |
| re_required_port_inout    | regex[^regex]        | regex required of `inout` port                  |
| re_required_port_input    | regex[^regex]        | regex required of `input` port                  |
| re_required_port_modport  | regex[^regex]        | regex required of `modport` port                |
| re_required_port_output   | regex[^regex]        | regex required of `output` port                 |
| re_required_reg           | regex[^regex]        | regex required of register type variable[^reg]  |
| re_required_struct        | regex[^regex]        | regex required of `struct`                      |
| re_required_wire          | regex[^regex]        | regex required of wire type variable[^wire]     |

[^casetype]: The available values are 

* `"snake"` -- snake_case
* `"screaming_snake"` -- SCREAMING_SNAKE_CASE
* `"lower_camel"` -- lowerCamelCase
* `"upper_camel"` -- UpperCamelCase

[^regex]: Regular expression string like `".*"`. The available syntax is [here](https://docs.rs/regex/latest/regex/#syntax).

[^reg]: Register type means that the variable is assigned in `always_ff`. It will be mapped to flip-flop in synthesis phase.

[^wire]: Wire type means that the variable is assigned in `always_comb`. It will be mapped to wire in synthesis phase.
