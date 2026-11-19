//! Runtime arguments given to `veryl test` with `--runtime-arg`.

use std::sync::Arc;

/// One `--runtime-arg`: `NAME` gives a flag (`value: None`), `NAME:VALUE`
/// a value.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RuntimeArg {
    pub name: String,
    pub value: Option<String>,
}

impl RuntimeArg {
    /// Splits `NAME[:VALUE]` at the first `:` (a name has none; a value may).
    pub fn parse(text: &str) -> Self {
        match text.split_once(':') {
            Some((name, value)) => Self {
                name: name.to_string(),
                value: Some(value.to_string()),
            },
            None => Self {
                name: text.to_string(),
                value: None,
            },
        }
    }
}

/// The runtime arguments of a run, as given, in order.
pub type RuntimeArgs = Arc<Vec<RuntimeArg>>;

/// The argument that wins for `name`: the last one given.
pub fn find<'a>(args: &'a [RuntimeArg], name: &str) -> Option<&'a RuntimeArg> {
    args.iter().rev().find(|x| x.name == name)
}

/// The winning argument of each name, in the order of their first
/// appearance (for passing as SystemVerilog plusargs).
pub fn winners(args: &[RuntimeArg]) -> Vec<&RuntimeArg> {
    let mut ret: Vec<&RuntimeArg> = Vec::new();
    for arg in args {
        if !ret.iter().any(|x| x.name == arg.name) {
            ret.push(find(args, &arg.name).unwrap());
        }
    }
    ret
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_find() {
        let args: Vec<_> = ["VERBOSE", "ROUNDS:4", "T:a:b", "ROUNDS", "E:"]
            .iter()
            .map(|x| RuntimeArg::parse(x))
            .collect();
        assert_eq!(args[0].value, None);
        assert_eq!(args[2].name, "T");
        assert_eq!(args[2].value.as_deref(), Some("a:b"));
        assert_eq!(args[4].value.as_deref(), Some(""));
        assert_eq!(find(&args, "ROUNDS").unwrap().value, None);
        assert!(find(&args, "X").is_none());

        let winners: Vec<_> = winners(&args)
            .iter()
            .map(|x| (x.name.as_str(), x.value.as_deref()))
            .collect();
        assert_eq!(
            winners,
            vec![
                ("VERBOSE", None),
                ("ROUNDS", None),
                ("T", Some("a:b")),
                ("E", Some(""))
            ]
        );
    }
}
