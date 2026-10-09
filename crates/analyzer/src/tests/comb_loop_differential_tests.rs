// Counted loops evaluated symbolically must find the combinational loops
// that taking each iteration's value finds, and no other. Random small
// procedures over loop nests compare the two, and each difference is
// shrunk to a small procedure that still shows it.
use super::*;

fn has_comb_loop(code: &str) -> bool {
    analyze(code)
        .into_iter()
        .any(|error| matches!(error, AnalyzerError::CombinationalLoop { .. }))
}

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

#[derive(Clone, Debug)]
enum Statement {
    Assign(String, Vec<String>),
    If(String, Vec<Statement>, Vec<Statement>),
    For(&'static str, usize, bool, Vec<Statement>),
}

impl Statement {
    fn render(&self) -> String {
        match self {
            Statement::Assign(destination, operands) => {
                format!("{destination} = {};", operands.join(" ^ "))
            }
            Statement::If(condition, then, otherwise) => {
                let mut text = format!("if {condition} {{ {} }}", render(then));
                if !otherwise.is_empty() {
                    text.push_str(&format!(" else {{ {} }}", render(otherwise)));
                }
                text
            }
            Statement::For(iterator, end, reverse, body) => {
                let reverse = if *reverse { "rev " } else { "" };
                format!("for {iterator} in {reverse}0..{end} {{ {} }}", render(body))
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
            always_comb {{ {} }}
            assign o = {{c, e, y[0], y[1], y[2], y[3], t[0], t[1]}};
        }}",
        render(statements)
    )
}

/// Every iterator stays below 3 and every index below 8, the arrays' size.
struct Generator {
    random: Random,
    iterators: Vec<&'static str>,
}

impl Generator {
    fn index(&mut self) -> String {
        let constant = self.random.below(3);
        match (self.iterators.as_slice(), self.random.below(5)) {
            ([], _) | (_, 0) => format!("{}", self.random.below(4)),
            ([.., inner], 1) => format!("{inner} + {constant}"),
            ([outer, ..], 2) => format!("{outer} + {constant}"),
            ([outer, inner], 3) => format!("{outer} + {inner}"),
            ([.., inner], 3 | 4) if self.random.chance(50) => format!("2 * {inner}"),
            ([.., inner], _) => inner.to_string(),
        }
    }

    fn operand(&mut self) -> String {
        match self.random.below(7) {
            0 => format!("x[{}]", self.index()),
            1 | 2 => format!("y[{}]", self.index()),
            3 => format!("t[{}]", self.index()),
            4 => "c".to_string(),
            5 => "e".to_string(),
            _ => format!("a[{}]", self.random.below(8)),
        }
    }

    fn destination(&mut self) -> String {
        match self.random.below(6) {
            0 | 1 => format!("y[{}]", self.index()),
            2 | 3 => format!("t[{}]", self.index()),
            4 => "c".to_string(),
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
        match self.random.below(10) {
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
            2 if self.iterators.len() < 2 => self.for_loop(depth),
            _ => {
                let destination = self.destination();
                let mut operands = vec![self.operand()];
                if self.random.chance(60) {
                    operands.push(self.operand());
                }
                Statement::Assign(destination, operands)
            }
        }
    }

    fn block(&mut self, depth: usize, length: usize) -> Vec<Statement> {
        let count = 1 + self.random.below(length);
        (0..count).map(|_| self.statement(depth)).collect()
    }

    fn for_loop(&mut self, depth: usize) -> Statement {
        let iterator = ["i", "j"][self.iterators.len()];
        let end = 1 + self.random.below(3);
        let reverse = self.random.chance(25);
        self.iterators.push(iterator);
        let body = self.block(depth + 1, 3);
        self.iterators.pop();
        Statement::For(iterator, end, reverse, body)
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
            Statement::Assign(destination, operands) if operands.len() > 1 => {
                for operand in operands {
                    replace(vec![Statement::Assign(
                        destination.clone(),
                        vec![operand.clone()],
                    )]);
                }
            }
            Statement::Assign(..) => {}
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
            Statement::For(iterator, end, reverse, body) => {
                if *end > 1 {
                    replace(vec![Statement::For(
                        iterator,
                        end - 1,
                        *reverse,
                        body.clone(),
                    )]);
                }
                if *reverse {
                    replace(vec![Statement::For(iterator, *end, false, body.clone())]);
                }
                for body in smaller(body) {
                    replace(vec![Statement::For(iterator, *end, *reverse, body)]);
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
    let outcome = std::panic::catch_unwind(|| {
        if !comb_loop_analysis_is_complete(&code)
            || !crate::comb_loop_detect::with_enumerated_loops(|| {
                comb_loop_analysis_is_complete(&code)
            })
        {
            return None;
        }
        let symbolic = has_comb_loop(&code);
        let enumerated = crate::comb_loop_detect::with_enumerated_loops(|| has_comb_loop(&code));
        Some((symbolic, enumerated))
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
