; Keywords
[
  "module"
  "interface"
  "package"
  "struct"
  "enum"
  "type"
  "const"
  "var"
  "let"
  "param"
  "modport"
  "inst"
  "generate"
  "import"
  "function"
  "embed"
  "return"
  "repeat"
  "as"
] @keyword

; Control flow keywords
[
  "if"
  "else"
  "for"
  "case"
  "switch"
  "default"
  "break"
  "in"
  "step"
  "rev"
  "if_reset"
] @keyword.control

; Operators
[
  "="
  "+="
  "-="
  "*="
  "/="
  "%="
  "&="
  "|="
  "^="
  "<<="
  ">>="
  "<<<="
  ">>>="
  "+"
  "-"
  "*"
  "/"
  "%"
  "**"
  "&"
  "|"
  "^"
  "~"
  "~&"
  "~|"
  "~^"
  "!"
  "&&"
  "||"
  "<<"
  ">>"
  "<<<"
  ">>>"
  "<:"
  "<="
  ">:"
  ">="
  "=="
  "!="
  "==?"
  "!=?"
  ".."
  "..="
  "::"
] @operator

; Block keywords
[
  "always_comb"
  "always_ff"
  "initial"
  "assign"
] @keyword.function

; Port directions
[
  "input"
  "output"
  "inout"
  "ref"
] @keyword.modifier

; Type modifiers
[
  "signed"
  "tri"
] @keyword.modifier

; Built-in types
[
  "logic"
  "bit"
  "u32"
  "u64"
  "i32"
  "clock"
  "reset"
  "string"
] @type.builtin

; Boolean literals
[
  "true"
  "false"
] @boolean

; Special literals
(special_literal) @constant.builtin

; Numbers
(number_literal) @number

; Strings
(string_literal) @string

; System functions
(system_function_identifier) @function.builtin
(scoped_system_function_identifier) @function.builtin

; Function calls
(call_expression
  (identifier) @function.call)

; Function declarations
(function_declaration
  (identifier) @function)

; Types
(builtin_type) @type.builtin
(identifier) @type
  (#match? @type "^[A-Z]")

; Module/interface/package names
(module_declaration
  (identifier) @module)

(interface_declaration
  (identifier) @module)

(package_declaration
  (identifier) @module)

; Enum members
(enum_member
  (identifier) @constant)

; Struct fields
(struct_field
  (identifier) @field)

; Parameters
(parameter_declaration
  (identifier) @parameter)

; Ports
(port_declaration
  (identifier) @variable.parameter)

; Variables
(variable_declaration
  (identifier) @variable)

(const_declaration
  (identifier) @constant)

; Type declarations
(type_declaration
  (identifier) @type.definition)

; Scoped identifiers
(scoped_identifier
  (identifier) @namespace)

; Import declarations
(import_declaration
  (scoped_identifier_or_glob) @namespace)

; Identifiers
(identifier) @variable

; Comments
(line_comment) @comment
(block_comment) @comment

; Attributes
(attribute
  (identifier) @attribute)

; Punctuation
[
  "("
  ")"
  "{"
  "}"
  "["
  "]"
] @punctuation.bracket

[
  ":"
  ";"
  ","
  "."
] @punctuation.delimiter

; Operators in expressions
"<" @punctuation.bracket
">" @punctuation.bracket

; Arrow operator
"->" @operator

; Embed content
(embed_content) @embedded
