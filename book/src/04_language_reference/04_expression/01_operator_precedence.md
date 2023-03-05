# Operator Precedence

In expression, operator precedence is almost the same as SystemVerilog.

|Operator                                                                              |Associativity|Precedence|
|--------------------------------------------------------------------------------------|-------------|----------|
|`()` `[]` `::` `.`                                                                    |Left         |Highest   |
|`+` `-` `!` `~` `&` `~&` <code>\|</code> <code>~\|</code> `^` `~^` `^~` (unary)       |Left         |          |
|`**`                                                                                  |Left         |          |
|`*` `/` `%`                                                                           |Left         |          |
|`+` `-` (binary)                                                                      |Left         |          |
|`<<` `>>` `<<<` `>>>`                                                                 |Left         |          |
|`<:` `<=` `>:` `>=`                                                                   |Left         |          |
|`==` `!=` `===` `!==` `==?` `!=?`                                                     |Left         |          |
|`&` (binary)                                                                          |Left         |          |
|`^` `~^` `^~` (binary)                                                                |Left         |          |
|<code>\|</code> (binary)                                                              |Left         |          |
|`&&`                                                                                  |Left         |          |
|<code>\|\|</code>                                                                     |Left         |          |
|`=` `+=` `-=` `*=` `/=` `%=` `&=` `^=` <code>\|=</code> <br> `<<=` `>>=` `<<<=` `>>>=`|None         |          |
|`{}`                                                                                  |None         |Lowest    |
