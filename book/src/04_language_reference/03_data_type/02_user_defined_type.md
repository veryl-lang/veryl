# User Defined Type

## Struct

`struct` is composit data type.
It can contain some fields, and these fields can be access through `.` operator.

```veryl,playground
module ModuleA {
    struct StructA {
        member_a: logic    ,
        member_b: logic<10>,
        member_c: u32      ,
    }

    var a: StructA;

    assign a.member_a = 0;
    assign a.member_b = 1;
    assign a.member_c = 2;
}
```

## Enum

`enum` is enumerable type.
It has some named variant, and the value of `enum` can be set to the one of them.
The variant name can be specified by `[enum name]::[variant name]`.
Each variant has the corresponding integral value.
The value can be specified by `=`.
Otherwise, it is assigned automatically.

```veryl,playground
module A {
    enum EnumA: logic<2> {
        member_a,
        member_b,
        member_c = 3,
    }

    var a: EnumA;

    assign a = EnumA::member_a;
}
```

## Typedef

The `type` keyword can be used to define a typedef to scalar or array types.

```veryl,playground
type word_t = logic<16>;
type regfile_t = word_t [16];
type octbyte = bit<8> [8];
```
