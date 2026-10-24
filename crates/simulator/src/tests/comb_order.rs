//! Generated combinational designs that are acyclic bit by bit and often
//! cyclic as whole variables or statements, written out in shuffled orders,
//! at the top level and inside a child instance.  Each must be accepted,
//! settle in one pass and agree with a reference model.

use super::*;

const INPUTS: usize = 4;

#[derive(Clone, Copy)]
enum Part {
    Whole,
    Lo,
    Hi,
}

/// A 4-bit slice: `(input, low bit)` or `(variable, low bit)`.
#[derive(Clone, Copy)]
enum Operand {
    Input(usize, usize),
    Var(usize, usize),
}

#[derive(Clone, Copy)]
enum Op {
    And,
    Or,
    Xor,
    Add,
}

#[derive(Clone, Copy)]
struct Term {
    a: Operand,
    op: Option<(Op, Operand)>,
}

/// A part's value: `lo` for a 4-bit part; `{hi, lo}` for a whole variable,
/// unless `wide` gives an 8-bit operation on whole sources instead.
#[derive(Clone, Copy)]
struct Rhs {
    wide: Option<(usize, Op, usize)>,
    hi: Option<Term>,
    lo: Term,
}

#[derive(Clone, Copy)]
enum Body {
    Plain(Rhs),
    /// `if source[bit] { then } else { else }`, a source numbered as in
    /// [`src8`].
    If(usize, usize, Rhs, Rhs),
}

#[derive(Clone, Copy)]
struct Node {
    var: usize,
    part: Part,
    body: Body,
}

struct Design {
    vars: usize,
    /// In evaluation order: every node reads only nodes before it.
    nodes: Vec<Node>,
    /// Statements as node indices; more than one makes an `always_comb`.
    stmts: Vec<Vec<usize>>,
}

/// A 4-bit window of an input or of ready bits, at any offset.
fn operand(rng: &mut Rng, ready: &[(usize, Part)]) -> Operand {
    let mut cands = Vec::new();
    for (&v, &m) in &full_mask(ready) {
        for lo in 0..=4 {
            if (m >> lo) & 0xf == 0xf {
                cands.push((v, lo));
            }
        }
    }
    if cands.is_empty() || rng.chance(25) {
        return Operand::Input(rng.below(INPUTS), rng.below(5));
    }
    let (v, lo) = cands[rng.below(cands.len())];
    Operand::Var(v, lo)
}

fn term(rng: &mut Rng, ready: &[(usize, Part)]) -> Term {
    let a = operand(rng, ready);
    let op = rng.chance(70).then(|| {
        let op = [Op::And, Op::Or, Op::Xor, Op::Add][rng.below(4)];
        (op, operand(rng, ready))
    });
    Term { a, op }
}

/// Per variable, the bits whose parts are ready.
fn full_mask(ready: &[(usize, Part)]) -> std::collections::BTreeMap<usize, u8> {
    let mut mask = std::collections::BTreeMap::<usize, u8>::new();
    for &(v, p) in ready {
        *mask.entry(v).or_default() |= match p {
            Part::Whole => 0xff,
            Part::Lo => 0x0f,
            Part::Hi => 0xf0,
        };
    }
    mask
}

/// A whole 8-bit source: inputs first, then variables every part of which is
/// ready.
fn src8(rng: &mut Rng, ready: &[(usize, Part)]) -> usize {
    let full: Vec<usize> = full_mask(ready)
        .into_iter()
        .filter(|(_, m)| *m == 0xff)
        .map(|(v, _)| v)
        .collect();
    if full.is_empty() || rng.chance(30) {
        rng.below(INPUTS)
    } else {
        INPUTS + full[rng.below(full.len())]
    }
}

/// A condition bit of an input or of a ready variable bit.
fn cond(rng: &mut Rng, ready: &[(usize, Part)]) -> (usize, usize) {
    let bits: Vec<(usize, usize)> = full_mask(ready)
        .into_iter()
        .flat_map(|(v, m)| {
            (0..8)
                .filter(move |b| (m >> b) & 1 == 1)
                .map(move |b| (INPUTS + v, b))
        })
        .collect();
    if bits.is_empty() || rng.chance(40) {
        (rng.below(INPUTS), rng.below(8))
    } else {
        bits[rng.below(bits.len())]
    }
}

fn rhs(rng: &mut Rng, part: Part, ready: &[(usize, Part)]) -> Rhs {
    if matches!(part, Part::Whole) && rng.chance(35) {
        let a = src8(rng, ready);
        let op = [Op::And, Op::Or, Op::Xor, Op::Add][rng.below(4)];
        let b = src8(rng, ready);
        let t = Term {
            a: Operand::Input(0, 0),
            op: None,
        };
        return Rhs {
            wide: Some((a, op, b)),
            hi: None,
            lo: t,
        };
    }
    let lo = term(rng, ready);
    let hi = matches!(part, Part::Whole).then(|| term(rng, ready));
    Rhs { wide: None, hi, lo }
}

fn generate(seed: u64) -> Design {
    let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    let vars = 4 + rng.below(6);
    let mut parts: Vec<(usize, Part)> = Vec::new();
    for var in 0..vars {
        if rng.chance(60) {
            parts.push((var, Part::Lo));
            parts.push((var, Part::Hi));
        } else {
            parts.push((var, Part::Whole));
        }
    }
    // A random evaluation order: interleaving the halves of different
    // variables is what makes them look cyclic as whole variables.
    rng.shuffle(&mut parts);
    let mut nodes = Vec::new();
    for (i, &(var, part)) in parts.iter().enumerate() {
        let ready = &parts[..i];
        let body = if rng.chance(25) {
            let then = rhs(&mut rng, part, ready);
            let other = rhs(&mut rng, part, ready);
            let (k, bit) = cond(&mut rng, ready);
            Body::If(k, bit, then, other)
        } else {
            Body::Plain(rhs(&mut rng, part, ready))
        };
        nodes.push(Node { var, part, body });
    }
    // Group runs of nodes into blocks.  A block keeps its nodes in evaluation
    // order, but spans the nodes another statement sits between.
    let mut order: Vec<usize> = (0..nodes.len()).collect();
    rng.shuffle(&mut order);
    let mut stmts: Vec<Vec<usize>> = Vec::new();
    let mut i = 0;
    while i < order.len() {
        let len = if rng.chance(40) { 2 + rng.below(3) } else { 1 };
        let mut group: Vec<usize> = order[i..(i + len).min(order.len())].to_vec();
        group.sort_unstable();
        stmts.push(group);
        i += len;
    }
    Design { vars, nodes, stmts }
}

fn target(node: &Node) -> String {
    match node.part {
        Part::Whole => format!("v{}", node.var),
        Part::Lo => format!("v{}[3:0]", node.var),
        Part::Hi => format!("v{}[7:4]", node.var),
    }
}

fn operand_text(o: Operand) -> String {
    match o {
        Operand::Input(k, lo) => format!("i{k}[{}:{lo}]", lo + 3),
        Operand::Var(v, lo) => format!("v{v}[{}:{lo}]", lo + 3),
    }
}

fn term_text(t: &Term) -> String {
    match t.op {
        None => operand_text(t.a),
        Some((op, b)) => {
            let op = match op {
                Op::And => "&",
                Op::Or => "|",
                Op::Xor => "^",
                Op::Add => "+",
            };
            format!("({} {op} {})", operand_text(t.a), operand_text(b))
        }
    }
}

fn src8_text(k: usize) -> String {
    if k < INPUTS {
        format!("i{k}")
    } else {
        format!("v{}", k - INPUTS)
    }
}

fn rhs_text(r: &Rhs) -> String {
    if let Some((a, op, b)) = r.wide {
        let op = match op {
            Op::And => "&",
            Op::Or => "|",
            Op::Xor => "^",
            Op::Add => "+",
        };
        return format!("({} {op} {})", src8_text(a), src8_text(b));
    }
    match &r.hi {
        None => term_text(&r.lo),
        Some(hi) => format!("{{{}, {}}}", term_text(hi), term_text(&r.lo)),
    }
}

fn node_text(node: &Node, indent: &str) -> String {
    let t = target(node);
    match &node.body {
        Body::Plain(r) => format!("{indent}{t} = {};\n", rhs_text(r)),
        Body::If(k, bit, a, b) => format!(
            "{indent}if {}[{bit}] {{\n{indent}    {t} = {};\n{indent}}} else {{\n{indent}    {t} = {};\n{indent}}}\n",
            src8_text(*k),
            rhs_text(a),
            rhs_text(b)
        ),
    }
}

/// The design as Veryl, its statements in the order `stmt_order` gives.
fn emit(design: &Design, stmt_order: &[usize]) -> String {
    let mut code = String::from("module Top (\n");
    for k in 0..INPUTS {
        code.push_str(&format!("    i{k}: input logic<8>,\n"));
    }
    for v in 0..design.vars {
        code.push_str(&format!("    o{v}: output logic<8>,\n"));
    }
    code.push_str(") {\n");
    for v in 0..design.vars {
        code.push_str(&format!("    var v{v}: logic<8>;\n"));
        code.push_str(&format!("    assign o{v} = v{v};\n"));
    }
    for &s in stmt_order {
        let group = &design.stmts[s];
        let single_plain =
            group.len() == 1 && matches!(design.nodes[group[0]].body, Body::Plain(_));
        if single_plain {
            code.push_str(&format!(
                "    assign {}",
                node_text(&design.nodes[group[0]], "").trim_start()
            ));
        } else {
            code.push_str("    always_comb {\n");
            for &n in group {
                code.push_str(&node_text(&design.nodes[n], "        "));
            }
            code.push_str("    }\n");
        }
    }
    code.push_str("}\n");
    code
}

fn eval_operand(o: Operand, inputs: &[u64], vars: &[u64]) -> u64 {
    match o {
        Operand::Input(k, lo) => (inputs[k] >> lo) & 0xf,
        Operand::Var(v, lo) => (vars[v] >> lo) & 0xf,
    }
}

fn eval_term(t: &Term, inputs: &[u64], vars: &[u64]) -> u64 {
    let a = eval_operand(t.a, inputs, vars);
    let r = match t.op {
        None => a,
        Some((op, b)) => {
            let b = eval_operand(b, inputs, vars);
            match op {
                Op::And => a & b,
                Op::Or => a | b,
                Op::Xor => a ^ b,
                Op::Add => a + b,
            }
        }
    };
    r & 0xf
}

fn eval_rhs(r: &Rhs, inputs: &[u64], vars: &[u64]) -> u64 {
    if let Some((a, op, b)) = r.wide {
        let g = |k: usize| {
            if k < INPUTS {
                inputs[k]
            } else {
                vars[k - INPUTS]
            }
        };
        let (a, b) = (g(a), g(b));
        return match op {
            Op::And => a & b,
            Op::Or => a | b,
            Op::Xor => a ^ b,
            Op::Add => a + b,
        } & 0xff;
    }
    let lo = eval_term(&r.lo, inputs, vars);
    match &r.hi {
        None => lo,
        Some(hi) => (eval_term(hi, inputs, vars) << 4) | lo,
    }
}

fn reference(design: &Design, inputs: &[u64]) -> Vec<u64> {
    let mut vars = vec![0u64; design.vars];
    for node in &design.nodes {
        let value = match &node.body {
            Body::Plain(r) => eval_rhs(r, inputs, &vars),
            Body::If(k, bit, a, b) => {
                let c = if *k < INPUTS {
                    inputs[*k]
                } else {
                    vars[*k - INPUTS]
                };
                if (c >> bit) & 1 == 1 {
                    eval_rhs(a, inputs, &vars)
                } else {
                    eval_rhs(b, inputs, &vars)
                }
            }
        };
        let v = &mut vars[node.var];
        *v = match node.part {
            Part::Whole => value,
            Part::Lo => (*v & 0xf0) | value,
            Part::Hi => (*v & 0x0f) | (value << 4),
        };
    }
    vars
}

/// The design inside an instance of `Sub`, ports passed straight through.
fn wrap_in_child(code: &str, design: &Design) -> String {
    let mut code = code.replacen("module Top (", "module Sub (", 1);
    let names: Vec<String> = (0..INPUTS)
        .map(|k| format!("i{k}"))
        .chain((0..design.vars).map(|v| format!("o{v}")))
        .collect();
    code.push_str("module Top (\n");
    for k in 0..INPUTS {
        code.push_str(&format!("    i{k}: input logic<8>,\n"));
    }
    for v in 0..design.vars {
        code.push_str(&format!("    o{v}: output logic<8>,\n"));
    }
    code.push_str(&format!(
        ") {{\n    inst u: Sub ({});\n}}\n",
        names.join(", ")
    ));
    code
}

/// Each design in three statement orders under each of `configs`: accepted,
/// one settle pass, and the reference model's values.
fn check_seeds(seeds: std::ops::Range<u64>, configs: &[Config], child: bool) {
    for seed in seeds {
        let design = generate(seed);
        let mut rng = Rng(seed + 1);
        for perm in 0..3 {
            let mut stmt_order: Vec<usize> = (0..design.stmts.len()).collect();
            if perm > 0 {
                rng.shuffle(&mut stmt_order);
            }
            let mut code = emit(&design, &stmt_order);
            if child {
                code = wrap_in_child(&code, &design);
            }
            for config in configs {
                let ir = match analyze_top(&code, config, "Top") {
                    Ok(ir) => ir,
                    Err(e) => panic!("seed {seed} order {perm} {config:?}: {e:?}\n{code}"),
                };
                assert_eq!(
                    ir.required_comb_passes, 1,
                    "seed {seed} order {perm} {config:?}\n{code}"
                );
                let mut sim = Simulator::new(ir, None);
                for _ in 0..4 {
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
                            "o{v}: seed {seed} order {perm} inputs {inputs:x?} {config:?}\n{code}"
                        );
                    }
                }
            }
        }
    }
}

fn interpreted() -> [Config; 2] {
    [
        Config::default(),
        Config {
            use_4state: true,
            ..Default::default()
        },
    ]
}

fn jit() -> [Config; 2] {
    [
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
    ($($name:ident: $range:expr, $configs:expr, $child:expr;)*) => {
        $(
            #[test]
            fn $name() {
                check_seeds($range, &$configs, $child);
            }
        )*
    };
}

seed_tests! {
    generated_comb_0: 0..64, interpreted(), false;
    generated_comb_1: 64..128, interpreted(), false;
    generated_comb_2: 128..192, interpreted(), false;
    generated_comb_3: 192..256, interpreted(), false;
    generated_comb_jit_0: 0..64, jit(), false;
    generated_comb_jit_1: 64..128, jit(), false;
    generated_comb_child_0: 0..64, interpreted(), true;
    generated_comb_child_1: 64..128, interpreted(), true;
    generated_comb_child_jit_0: 0..64, jit(), true;
    generated_comb_child_jit_1: 64..128, jit(), true;
}
