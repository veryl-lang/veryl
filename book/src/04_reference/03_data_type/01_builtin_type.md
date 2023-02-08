# Builtin Type

## 4-state data type which has variable width

`logic` is 4-state (`0`, `1`, `x`, `z`) data type.
The variable width can be specified by `<>` after `logic`.
Multi-dimentional can be specified by `<X, Y, Z,,,>`.

```veryl,playground
module ModuleA {
    var _a: logic;
    var _b: logic<10>;
    var _c: logic<10, 10>;
}
```

## 2-state data type which has variable width

`bit` is 2-state (`0`, `1`) data type.
The variable width can be specified by `<>` after `bit`.
Multi-dimentional can be specified by `<X, Y, Z,,,>`.

```veryl,playground
module ModuleA {
    var _a: bit;
    var _b: bit<10>;
    var _c: bit<10, 10>;
}
```

## Integer type

There are some integer types:

* `u32`: 32bit unsigned integer
* `u64`: 64bit unsigned integer
* `i32`: 32bit signed integer
* `i64`: 64bit signed integer

```veryl,playground
module ModuleA {
    var _a: u32;
    var _b: u64;
    var _c: i32;
    var _d: i64;
}
```

## Floating point type

There are some floating point types:

* `f32`: 32bit floating point
* `f64`: 64bit floating point

Both of them are represented as described by IEEE Std 754.

```veryl,playground
module ModuleA {
    var _a: u32;
    var _b: u64;
    var _c: i32;
    var _d: i64;
}
```

## String type

`string` is string type.

```veryl,playground
module ModuleA {
    var _a: string;
}
```

## Type type

`type` is a type which represents type kind.
Variable of `type` can be defined as `parameter` or `localparam` only.

```veryl,playground
module ModuleA {
    localparam a: type = logic;
    localparam b: type = logic<10>;
    localparam c: type = u32;
}
```
