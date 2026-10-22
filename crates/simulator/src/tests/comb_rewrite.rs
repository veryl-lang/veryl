//! Generated combinational designs whose `always_comb` blocks write the parts
//! they own several times: rewrites of a whole part, of a 2-bit slice of it,
//! under a guard with or without an else, read back in between.  Other blocks
//! read a part only once its block has finished with it, so every design is
//! acyclic bit by bit.  Each must be accepted, settle in one pass and agree
//! with a reference model that runs the writes in their block order.

use super::*;

const INPUTS: usize = 4;

#[derive(Clone, Copy)]
enum Src {
    Input(usize),
    Var(usize),
}

/// `(source, low bit, width)`.
#[derive(Clone, Copy)]
struct Operand(Src, usize, usize);

/// `a`, or `a op b` with `op` indexing `& | ^ +`.
#[derive(Clone, Copy)]
struct Value2(Operand, Option<(usize, Operand)>);

struct Write {
    var: usize,
    hi: usize,
    lo: usize,
    value: Value2,
    /// `if input[bit] { value } else { otherwise }`; no else keeps the bits.
    guard: Option<(usize, usize, Option<Value2>)>,
}

struct Design {
    vars: usize,
    /// In evaluation order.
    writes: Vec<Write>,
    /// Each block's writes, in evaluation order.
    blocks: Vec<Vec<usize>>,
}

fn generate(seed: u64) -> Design {
    let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    let vars = 3 + rng.below(4);
    // `(variable, hi, lo)`: whole variables and halves.
    let mut parts: Vec<(usize, usize, usize)> = Vec::new();
    for v in 0..vars {
        if rng.chance(60) {
            parts.push((v, 3, 0));
            parts.push((v, 7, 4));
        } else {
            parts.push((v, 7, 0));
        }
    }
    let block_count = 2 + rng.below(3);
    let owner: Vec<usize> = parts.iter().map(|_| rng.below(block_count)).collect();
    let mut slots: Vec<usize> = Vec::new();
    for part in 0..parts.len() {
        let n = if rng.chance(55) { 2 + rng.below(2) } else { 1 };
        slots.extend(std::iter::repeat_n(part, n));
    }
    rng.shuffle(&mut slots);
    let total: Vec<usize> = (0..parts.len())
        .map(|p| slots.iter().filter(|&&s| s == p).count())
        .collect();
    let mut done = vec![0usize; parts.len()];
    let mut writes = Vec::new();
    let mut blocks = vec![Vec::new(); block_count];
    for &part in &slots {
        let (var, part_hi, part_lo) = parts[part];
        let block = owner[part];
        // Its own block's parts once written, other blocks' parts once final.
        let ready: Vec<(usize, usize, usize)> = parts
            .iter()
            .enumerate()
            .filter(|&(q, _)| {
                if owner[q] == block {
                    done[q] > 0
                } else {
                    done[q] == total[q]
                }
            })
            .map(|(_, &p)| p)
            .collect();
        let first = done[part] == 0;
        let (hi, lo) = if !first && rng.chance(35) {
            let part_width = part_hi - part_lo + 1;
            let base = part_lo + 2 * rng.below(part_width / 2);
            (base + 1, base)
        } else {
            (part_hi, part_lo)
        };
        let width = hi - lo + 1;
        let operand = |rng: &mut Rng| -> Operand {
            let input =
                |rng: &mut Rng| Operand(Src::Input(rng.below(INPUTS)), rng.below(9 - width), width);
            if ready.is_empty() || rng.chance(30) {
                return input(rng);
            }
            let (v, q_hi, q_lo) = ready[rng.below(ready.len())];
            let q_width = q_hi - q_lo + 1;
            if q_width >= width {
                Operand(Src::Var(v), q_lo + rng.below(q_width - width + 1), width)
            } else {
                input(rng)
            }
        };
        let value = |rng: &mut Rng| -> Value2 {
            let a = operand(rng);
            Value2(a, rng.chance(60).then(|| (rng.below(4), operand(rng))))
        };
        let v = value(&mut rng);
        // A first write needs an else: without one its bits would latch.
        let guard = rng.chance(35).then(|| {
            let (k, bit) = (rng.below(INPUTS), rng.below(8));
            let otherwise = (first || rng.chance(50)).then(|| value(&mut rng));
            (k, bit, otherwise)
        });
        blocks[block].push(writes.len());
        writes.push(Write {
            var,
            hi,
            lo,
            value: v,
            guard,
        });
        done[part] += 1;
    }
    Design {
        vars,
        writes,
        blocks,
    }
}

fn operand_text(o: &Operand) -> String {
    let Operand(src, lo, width) = *o;
    let name = match src {
        Src::Input(k) => format!("i{k}"),
        Src::Var(v) => format!("v{v}"),
    };
    format!("{name}[{}:{lo}]", lo + width - 1)
}

fn value_text(v: &Value2) -> String {
    match &v.1 {
        None => operand_text(&v.0),
        Some((op, b)) => format!(
            "({} {} {})",
            operand_text(&v.0),
            ["&", "|", "^", "+"][*op],
            operand_text(b)
        ),
    }
}

fn emit(design: &Design) -> String {
    let mut code = String::from("module Top (\n");
    for k in 0..INPUTS {
        code.push_str(&format!("    i{k}: input logic<8>,\n"));
    }
    for v in 0..design.vars {
        code.push_str(&format!("    o{v}: output logic<8>,\n"));
    }
    code.push_str(") {\n");
    for v in 0..design.vars {
        code.push_str(&format!(
            "    var v{v}: logic<8>;\n    assign o{v} = v{v};\n"
        ));
    }
    for block in design.blocks.iter().filter(|b| !b.is_empty()) {
        code.push_str("    always_comb {\n");
        for &w in block {
            let x = &design.writes[w];
            let target = format!("v{}[{}:{}]", x.var, x.hi, x.lo);
            match &x.guard {
                None => code.push_str(&format!("        {target} = {};\n", value_text(&x.value))),
                Some((k, bit, otherwise)) => {
                    code.push_str(&format!(
                        "        if i{k}[{bit}] {{\n            {target} = {};\n        }}",
                        value_text(&x.value)
                    ));
                    if let Some(o) = otherwise {
                        code.push_str(&format!(
                            " else {{\n            {target} = {};\n        }}",
                            value_text(o)
                        ));
                    }
                    code.push('\n');
                }
            }
        }
        code.push_str("    }\n");
    }
    code.push_str("}\n");
    code
}

fn eval_operand(o: &Operand, inputs: &[u64], vars: &[u64]) -> u64 {
    let x = match o.0 {
        Src::Input(k) => inputs[k],
        Src::Var(v) => vars[v],
    };
    (x >> o.1) & ((1 << o.2) - 1)
}

fn eval_value(v: &Value2, width: usize, inputs: &[u64], vars: &[u64]) -> u64 {
    let a = eval_operand(&v.0, inputs, vars);
    let r = match &v.1 {
        None => a,
        Some((op, b)) => {
            let b = eval_operand(b, inputs, vars);
            match op {
                0 => a & b,
                1 => a | b,
                2 => a ^ b,
                _ => a + b,
            }
        }
    };
    r & ((1 << width) - 1)
}

fn reference(design: &Design, inputs: &[u64]) -> Vec<u64> {
    let mut vars = vec![0u64; design.vars];
    for x in &design.writes {
        let width = x.hi - x.lo + 1;
        let value = match &x.guard {
            None => Some(eval_value(&x.value, width, inputs, &vars)),
            Some((k, bit, otherwise)) => {
                if (inputs[*k] >> bit) & 1 == 1 {
                    Some(eval_value(&x.value, width, inputs, &vars))
                } else {
                    otherwise
                        .as_ref()
                        .map(|o| eval_value(o, width, inputs, &vars))
                }
            }
        };
        if let Some(value) = value {
            let mask = ((1u64 << width) - 1) << x.lo;
            vars[x.var] = (vars[x.var] & !mask) | (value << x.lo);
        }
    }
    vars
}

/// Each design under each of `configs`: accepted, one settle pass, and the
/// reference model's values.
fn check_seeds(seeds: std::ops::Range<u64>, configs: &[Config]) {
    for seed in seeds {
        let design = generate(seed);
        let code = emit(&design);
        let mut rng = Rng(seed + 7);
        for config in configs {
            let ir = match analyze_top(&code, config, "Top") {
                Ok(ir) => ir,
                Err(e) => panic!("seed {seed} {config:?}: {e:?}\n{code}"),
            };
            assert_eq!(ir.required_comb_passes, 1, "seed {seed} {config:?}\n{code}");
            let mut sim = Simulator::new(ir, None);
            for _ in 0..6 {
                let inputs: Vec<u64> = (0..INPUTS).map(|_| rng.next() & 0xff).collect();
                for (k, &x) in inputs.iter().enumerate() {
                    sim.set(&format!("i{k}"), Value::new(x, 8, false));
                }
                sim.step(&Event::clock(VarId::SYNTHETIC));
                let want = reference(&design, &inputs);
                for (v, &w) in want.iter().enumerate() {
                    assert_eq!(
                        sim.get(&format!("o{v}")).unwrap(),
                        Value::new(w, 8, false),
                        "o{v}: seed {seed} inputs {inputs:x?} {config:?}\n{code}"
                    );
                }
            }
        }
    }
}

fn configs() -> [Config; 4] {
    [
        Config::default(),
        Config {
            use_4state: true,
            ..Default::default()
        },
        Config {
            use_jit: true,
            ..Default::default()
        },
        Config {
            use_jit: true,
            use_4state: true,
            ..Default::default()
        },
    ]
}

// Split so the harness runs the ranges in parallel.
macro_rules! seed_tests {
    ($($name:ident: $range:expr,)*) => {
        $(
            #[test]
            fn $name() {
                check_seeds($range, &configs());
            }
        )*
    };
}

seed_tests! {
    generated_rewrites_0: 0..64,
    generated_rewrites_1: 64..128,
    generated_rewrites_2: 128..192,
    generated_rewrites_3: 192..256,
}
