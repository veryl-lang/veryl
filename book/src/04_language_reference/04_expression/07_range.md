# Range

Range can be specified through range operator. There are two range operator:

* `..`: half-open interval
* `..=`: closed interval

Range can be used at some description like `for` statement.

```veryl,playground
module ModuleA {
    initial {
        for _i: u32 in 0..10 {
        }

        for _j: u32 in 0..=10 {
        }
    }
}
```
