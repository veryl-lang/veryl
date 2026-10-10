// Counted loops evaluated symbolically must find the combinational loops
// that taking each iteration's value finds, and no other. Random small
// procedures over loop nests compare the two, and each difference is
// shrunk to a small procedure that still shows it.
use super::*;

/// A small deterministic generator, so that a failing case reproduces from
/// its seed.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }

    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }
}

/// How a loop takes its values. A step that is not one takes each value
/// in turn, as do the loops around a counted one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Range {
    Ascending,
    Descending,
    /// `1, 2, 4, ...`
    Doubling,
    /// `0, 2, 4, ...`
    Skipping,
}

#[derive(Clone, Debug)]
enum Statement {
    /// A destination, its operands and the operator between them.
    Assign(String, Vec<String>, &'static str),
    If(String, Vec<Statement>, Vec<Statement>),
    /// A selector and the bodies of its values `0`, `1` and the default.
    Case(String, Vec<Vec<Statement>>),
    For(&'static str, usize, Range, Vec<Statement>),
}

impl Statement {
    fn render(&self) -> String {
        match self {
            Statement::Assign(destination, operands, operator) => {
                format!(
                    "{destination} = {};",
                    operands.join(&format!(" {operator} "))
                )
            }
            Statement::Case(selector, arms) => {
                let labels = ["0", "1", "default"];
                let arms = arms
                    .iter()
                    .zip(labels)
                    .map(|(body, label)| format!("{label}: {{ {} }}", render(body)))
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("case {selector} {{ {arms} }}")
            }
            Statement::If(condition, then, otherwise) => {
                let mut text = format!("if {condition} {{ {} }}", render(then));
                if !otherwise.is_empty() {
                    text.push_str(&format!(" else {{ {} }}", render(otherwise)));
                }
                text
            }
            Statement::For(iterator, end, range, body) => {
                let range = match range {
                    Range::Ascending => format!("0..{end}"),
                    Range::Descending => format!("rev 0..{end}"),
                    Range::Doubling => format!("1..{end} step *= 2"),
                    Range::Skipping => format!("0..{end} step += 2"),
                };
                format!("for {iterator} in {range} {{ {} }}", render(body))
            }
        }
    }
}

fn render(statements: &[Statement]) -> String {
    statements
        .iter()
        .map(Statement::render)
        .collect::<Vec<_>>()
        .join(" ")
}

fn module(statements: &[Statement]) -> String {
    format!(
        "module Top (a: input logic<8>, o: output logic<8>) {{
            var x: logic [8];
            var y: logic [8];
            var t: logic [8];
            var c: logic;
            var e: logic;
            var p: logic<8>;
            function f (v: input logic, w: input logic) -> logic {{
                var s: logic [4];
                s[0] = v;
                for m in 0..3 {{ s[m + 1] = s[m] ^ w; }}
                return s[3];
            }}
            function g (v: input logic, w: input logic) -> logic {{
                var r: logic;
                r = w;
                for m in 0..2 {{ if v {{ r = r ^ w; }} }}
                return r;
            }}
            always_comb {{ {} }}
            assign o = {{c, e, y[0], y[1], y[2], t[0], t[1], p[1]}};
        }}",
        render(statements)
    )
}

/// Every iterator stays below 3 and every index below 8, the arrays' and
/// `p`'s size.
struct Generator {
    random: Random,
    iterators: Vec<&'static str>,
}

impl Generator {
    fn index(&mut self) -> String {
        let constant = self.random.below(3);
        let iterators = self.iterators.clone();
        match (iterators.as_slice(), self.random.below(6)) {
            ([], _) | (_, 0) => format!("{}", self.random.below(4)),
            ([.., inner], 1) => format!("{inner} + {constant}"),
            ([outer, ..], 2) => format!("{outer} + {constant}"),
            ([outer, .., inner], 3) => format!("{outer} + {inner}"),
            ([first, second, third], 5) => format!("{first} + {second} + {third}"),
            ([.., inner], 3..=5) if self.random.chance(50) => format!("2 * {inner}"),
            ([.., inner], _) => inner.to_string(),
        }
    }

    fn operand(&mut self) -> String {
        match self.random.below(10) {
            0 => format!("x[{}]", self.index()),
            1 | 2 => format!("y[{}]", self.index()),
            3 => format!("t[{}]", self.index()),
            4 => format!("p[{}]", self.index()),
            5 => "c".to_string(),
            6 => "e".to_string(),
            7 if self.random.chance(50) => {
                let (left, right) = (self.simple_operand(), self.simple_operand());
                let function = ["f", "g"][self.random.below(2)];
                format!("{function}({left}, {right})")
            }
            _ => format!("a[{}]", self.random.below(8)),
        }
    }

    /// An operand without a call, as a call's actual.
    fn simple_operand(&mut self) -> String {
        match self.random.below(5) {
            0 => format!("y[{}]", self.index()),
            1 => format!("t[{}]", self.index()),
            2 => "c".to_string(),
            3 => "e".to_string(),
            _ => format!("p[{}]", self.index()),
        }
    }

    fn destination(&mut self) -> String {
        match self.random.below(7) {
            0 | 1 => format!("y[{}]", self.index()),
            2 | 3 => format!("t[{}]", self.index()),
            4 => format!("p[{}]", self.index()),
            5 => "c".to_string(),
            _ => "e".to_string(),
        }
    }

    fn condition(&mut self) -> String {
        match (self.iterators.as_slice(), self.random.below(3)) {
            ([.., inner], 0) => format!("{inner} == {}", self.random.below(3)),
            ([outer, ..], 1) => format!("{outer} >= {}", self.random.below(3)),
            _ => format!("a[{}]", self.random.below(8)),
        }
    }

    fn statement(&mut self, depth: usize) -> Statement {
        match self.random.below(11) {
            10 if depth < 3 => {
                let selector = match self.iterators.last() {
                    Some(iterator) if self.random.chance(30) => iterator.to_string(),
                    _ => format!("a[{}:{}]", 2 + self.random.below(6), self.random.below(2)),
                };
                let arms = (0..3).map(|_| self.block(depth + 1, 2)).collect();
                Statement::Case(selector, arms)
            }
            0 | 1 if depth < 3 => {
                let condition = self.condition();
                let then = self.block(depth + 1, 2);
                let otherwise = if self.random.chance(50) {
                    self.block(depth + 1, 2)
                } else {
                    Vec::new()
                };
                Statement::If(condition, then, otherwise)
            }
            2 if self.iterators.len() < 3 => self.for_loop(depth),
            _ => {
                let destination = self.destination();
                let mut operands = vec![self.operand()];
                if self.random.chance(60) {
                    operands.push(self.operand());
                }
                let operator = ["^", "^", "&", "|", "+"][self.random.below(5)];
                Statement::Assign(destination, operands, operator)
            }
        }
    }

    fn block(&mut self, depth: usize, length: usize) -> Vec<Statement> {
        let count = 1 + self.random.below(length);
        (0..count).map(|_| self.statement(depth)).collect()
    }

    fn for_loop(&mut self, depth: usize) -> Statement {
        let iterator = ["i", "j", "k"][self.iterators.len()];
        let end = 1 + self.random.below(3);
        let range = match self.random.below(8) {
            0 | 1 => Range::Descending,
            2 => Range::Doubling,
            3 => Range::Skipping,
            _ => Range::Ascending,
        };
        self.iterators.push(iterator);
        let body = self.block(depth + 1, 3);
        self.iterators.pop();
        Statement::For(iterator, end, range, body)
    }

    fn generate(seed: u64) -> Vec<Statement> {
        let mut generator = Generator {
            random: Random(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1),
            iterators: Vec::new(),
        };
        let mut statements = generator.block(0, 2);
        statements.push(generator.for_loop(0));
        if generator.random.chance(50) {
            statements.extend(generator.block(0, 2));
        }
        statements
    }
}

/// Each procedure one step smaller than `statements`: one statement
/// removed, a branch replaced by one of its sides, or a loop shortened.
fn smaller(statements: &[Statement]) -> Vec<Vec<Statement>> {
    let mut candidates = Vec::new();
    for (index, statement) in statements.iter().enumerate() {
        let mut without = statements.to_vec();
        without.remove(index);
        candidates.push(without);
        let mut replace = |replacements: Vec<Statement>| {
            let mut replaced = statements[..index].to_vec();
            replaced.extend(replacements);
            replaced.extend_from_slice(&statements[index + 1..]);
            candidates.push(replaced);
        };
        match statement {
            Statement::Assign(destination, operands, operator) if operands.len() > 1 => {
                for operand in operands {
                    replace(vec![Statement::Assign(
                        destination.clone(),
                        vec![operand.clone()],
                        operator,
                    )]);
                }
            }
            Statement::Assign(..) => {}
            Statement::Case(selector, arms) => {
                for arm in arms {
                    replace(arm.clone());
                }
                for (position, arm) in arms.iter().enumerate() {
                    for smaller_arm in smaller(arm) {
                        let mut arms = arms.clone();
                        arms[position] = smaller_arm;
                        replace(vec![Statement::Case(selector.clone(), arms)]);
                    }
                }
            }
            Statement::If(condition, then, otherwise) => {
                replace(then.clone());
                replace(otherwise.clone());
                for then in smaller(then) {
                    replace(vec![Statement::If(
                        condition.clone(),
                        then,
                        otherwise.clone(),
                    )]);
                }
                for otherwise in smaller(otherwise) {
                    replace(vec![Statement::If(
                        condition.clone(),
                        then.clone(),
                        otherwise,
                    )]);
                }
            }
            Statement::For(iterator, end, range, body) => {
                if *end > 1 {
                    replace(vec![Statement::For(
                        iterator,
                        end - 1,
                        *range,
                        body.clone(),
                    )]);
                }
                if *range != Range::Ascending {
                    replace(vec![Statement::For(
                        iterator,
                        *end,
                        Range::Ascending,
                        body.clone(),
                    )]);
                }
                for body in smaller(body) {
                    replace(vec![Statement::For(iterator, *end, *range, body)]);
                }
            }
        }
    }
    candidates
}

/// The symbolic and the enumerated evaluation of one procedure: `None` when
/// either is incomplete, else whether each finds a loop.
fn compare(statements: &[Statement]) -> Option<(bool, bool)> {
    let code = module(statements);
    // Each side's loops and completeness come from one analysis.
    let outcome = std::panic::catch_unwind(|| {
        let (symbolic, complete) = comb_loop_outcome(&code);
        let (enumerated, enumerated_complete) =
            crate::comb_loop_detect::with_enumerated_loops(|| comb_loop_outcome(&code));
        (complete && enumerated_complete).then_some((symbolic, enumerated))
    });
    outcome.ok().flatten()
}

/// The smallest procedure reachable by `smaller` that still differs as
/// `statements` does.
fn shrink(mut statements: Vec<Statement>, difference: (bool, bool)) -> Vec<Statement> {
    'shrinking: loop {
        for candidate in smaller(&statements) {
            if compare(&candidate) == Some(difference) {
                statements = candidate;
                continue 'shrinking;
            }
        }
        return statements;
    }
}

#[test]
#[ignore = "reports the differences of the symbolic evaluation from enumeration"]
fn counted_loops_agree_with_enumeration() {
    fn variable<T: std::str::FromStr>(name: &str) -> Option<T> {
        std::env::var(name)
            .ok()
            .and_then(|value| value.parse().ok())
    }
    let cases = variable("VERYL_DIFFERENTIAL_CASES").unwrap_or(300u64);
    let first = variable("VERYL_DIFFERENTIAL_SEED").unwrap_or(0u64);
    let shown = variable("VERYL_DIFFERENTIAL_SHOWN").unwrap_or(5usize);
    let mut false_loops = Vec::new();
    let mut missed_loops = Vec::new();
    let mut incomplete = 0;
    for seed in first..first + cases {
        let statements = Generator::generate(seed);
        match compare(&statements) {
            None => {
                if incomplete < shown {
                    println!("incomplete, seed {seed}: {}", render(&statements));
                }
                incomplete += 1;
            }
            Some((true, false)) => false_loops.push((seed, statements)),
            Some((false, true)) => missed_loops.push((seed, statements)),
            Some(_) => {}
        }
    }
    for (kind, difference, cases) in [
        ("false loop", (true, false), &false_loops),
        ("missed loop", (false, true), &missed_loops),
    ] {
        for (seed, statements) in cases.iter().take(shown) {
            let shrunk = shrink(statements.clone(), difference);
            println!("{kind}, seed {seed}: {}", render(&shrunk));
        }
    }
    println!(
        "{cases} cases: {} false loops, {} missed loops, {incomplete} incomplete",
        false_loops.len(),
        missed_loops.len()
    );
    assert!(
        false_loops.is_empty() && missed_loops.is_empty(),
        "{} false loops, {} missed loops",
        false_loops.len(),
        missed_loops.len()
    );
}

/// The analysis of a procedure does not depend on what was parsed before
/// it: each analysis of the same code takes new token identities, which
/// must not change the order the analysis works in, and so neither its
/// loops nor whether it completes.
#[test]
fn analyses_of_one_procedure_agree() {
    let cases = std::env::var("VERYL_DETERMINISM_CASES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(60u64);
    for seed in 0..cases {
        let code = module(&Generator::generate(seed));
        let first = comb_loop_outcome(&code);
        let second = comb_loop_outcome(&code);
        assert_eq!(first, second, "seed {seed}: {code}");
    }
}
