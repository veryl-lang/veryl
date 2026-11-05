//! Generated combinational designs over 16-bit variables written by the
//! nibble, half or whole, arrays of 4-bit elements at constant indices, and
//! `case` statements, read back through windows at any bit offset.  Every
//! design is acyclic bit by bit.  Each must be accepted, settle in one pass
//! and agree with a reference model, in two statement orders.

use super::*;

const INPUTS: usize = 4;

/// What one statement writes.
#[derive(Clone, Copy)]
enum Target {
    /// `(variable, nibbles, low nibble)`: a nibble, a half or the whole.
    Var(usize, usize, usize),
    /// `(array, element)`.
    Elem(usize, usize),
}

/// A window read: `(input or variable, low bit, width)`, or a whole element.
#[derive(Clone, Copy)]
enum Src {
    Input(usize, usize, usize),
    Var(usize, usize, usize),
    Elem(usize, usize),
}

/// `a`, or `a op b` with `op` indexing `& | ^ +`.
#[derive(Clone)]
struct Expr(Src, Option<(usize, Src)>);

#[derive(Clone)]
enum Body {
    Plain(Expr),
    /// `if input[bit] { then } else { else }`.
    If((usize, usize), Expr, Expr),
    /// `case selector { 0..: arms, default }` over a 2-bit selector.
    Case(Src, Vec<Expr>, Expr),
}

struct Node {
    target: Target,
    body: Body,
}

struct Design {
    vars: usize,
    arrays: usize,
    nodes: Vec<Node>,
    stmts: Vec<Vec<usize>>,
}

fn width_of(t: Target) -> usize {
    match t {
        Target::Var(_, n, _) => 4 * n,
        Target::Elem(..) => 4,
    }
}

fn generate(seed: u64) -> Design {
    let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    let vars = 2 + rng.below(4);
    let arrays = 1 + rng.below(2);
    let mut targets: Vec<Target> = Vec::new();
    for v in 0..vars {
        match rng.below(3) {
            0 => targets.push(Target::Var(v, 4, 0)),
            1 => {
                targets.push(Target::Var(v, 2, 0));
                targets.push(Target::Var(v, 2, 2));
            }
            _ => {
                for q in 0..4 {
                    targets.push(Target::Var(v, 1, q));
                }
            }
        }
    }
    for a in 0..arrays {
        for e in 0..4 {
            targets.push(Target::Elem(a, e));
        }
    }
    rng.shuffle(&mut targets);
    let mut nodes = Vec::new();
    for i in 0..targets.len() {
        let ready = &targets[..i];
        // Per variable, the nibbles already written.
        let mut mask = vec![0u8; vars];
        let mut elems: Vec<(usize, usize)> = Vec::new();
        for t in ready {
            match *t {
                Target::Var(v, n, lo) => {
                    for q in lo..lo + n {
                        mask[v] |= 1 << q;
                    }
                }
                Target::Elem(a, e) => elems.push((a, e)),
            }
        }
        let w = width_of(targets[i]);
        let src = |rng: &mut Rng, w: usize| -> Src {
            let mut cands: Vec<Src> = Vec::new();
            for (v, &written) in mask.iter().enumerate() {
                for lo in 0..=(16 - w) {
                    let need: u32 = ((1u32 << w) - 1) << lo;
                    let nib_need = (0..4)
                        .filter(|q| need >> (4 * q) & 0xf != 0)
                        .fold(0u8, |m, q| m | 1 << q);
                    if written & nib_need == nib_need {
                        cands.push(Src::Var(v, lo, w));
                    }
                }
            }
            if w == 4 {
                for &(a, e) in &elems {
                    cands.push(Src::Elem(a, e));
                }
            }
            if cands.is_empty() || rng.chance(25) {
                Src::Input(rng.below(INPUTS), rng.below(17 - w), w)
            } else {
                cands[rng.below(cands.len())]
            }
        };
        let expr = |rng: &mut Rng| -> Expr {
            let a = src(rng, w);
            let b = rng.chance(60).then(|| (rng.below(4), src(rng, w)));
            Expr(a, b)
        };
        let body = match rng.below(4) {
            0 => {
                let t = expr(&mut rng);
                let e = expr(&mut rng);
                Body::If((rng.below(INPUTS), rng.below(16)), t, e)
            }
            1 => {
                let sel = src(&mut rng, 2);
                let arms = (0..3).map(|_| expr(&mut rng)).collect();
                let d = expr(&mut rng);
                Body::Case(sel, arms, d)
            }
            _ => Body::Plain(expr(&mut rng)),
        };
        nodes.push(Node {
            target: targets[i],
            body,
        });
    }
    let mut order: Vec<usize> = (0..nodes.len()).collect();
    rng.shuffle(&mut order);
    let mut stmts: Vec<Vec<usize>> = Vec::new();
    let mut i = 0;
    while i < order.len() {
        let len = if rng.chance(45) { 2 + rng.below(3) } else { 1 };
        let mut g: Vec<usize> = order[i..(i + len).min(order.len())].to_vec();
        g.sort_unstable();
        stmts.push(g);
        i += len;
    }
    Design {
        vars,
        arrays,
        nodes,
        stmts,
    }
}

fn src_text(s: Src) -> String {
    match s {
        Src::Input(k, lo, w) => format!("i{k}[{}:{lo}]", lo + w - 1),
        Src::Var(v, lo, w) => format!("v{v}[{}:{lo}]", lo + w - 1),
        Src::Elem(a, e) => format!("a{a}[{e}]"),
    }
}

fn expr_text(x: &Expr) -> String {
    match &x.1 {
        None => src_text(x.0),
        Some((op, b)) => format!(
            "({} {} {})",
            src_text(x.0),
            ["&", "|", "^", "+"][*op],
            src_text(*b)
        ),
    }
}

fn target_text(t: Target) -> String {
    match t {
        Target::Var(v, n, lo) => format!("v{v}[{}:{}]", 4 * (lo + n) - 1, 4 * lo),
        Target::Elem(a, e) => format!("a{a}[{e}]"),
    }
}

fn node_text(n: &Node, ind: &str) -> String {
    let t = target_text(n.target);
    match &n.body {
        Body::Plain(x) => format!("{ind}{t} = {};\n", expr_text(x)),
        Body::If((k, b), x, y) => format!(
            "{ind}if i{k}[{b}] {{\n{ind}    {t} = {};\n{ind}}} else {{\n{ind}    {t} = {};\n{ind}}}\n",
            expr_text(x),
            expr_text(y)
        ),
        Body::Case(sel, arms, d) => {
            let mut s = format!("{ind}case {} {{\n", src_text(*sel));
            for (i, a) in arms.iter().enumerate() {
                s.push_str(&format!("{ind}    2'd{i}: {t} = {};\n", expr_text(a)));
            }
            s.push_str(&format!(
                "{ind}    default: {t} = {};\n{ind}}}\n",
                expr_text(d)
            ));
            s
        }
    }
}

fn emit(d: &Design, order: &[usize]) -> String {
    let mut c = String::from("module Top (\n");
    for k in 0..INPUTS {
        c.push_str(&format!("    i{k}: input logic<16>,\n"));
    }
    for v in 0..d.vars {
        c.push_str(&format!("    o{v}: output logic<16>,\n"));
    }
    for a in 0..d.arrays {
        c.push_str(&format!("    p{a}: output logic<16>,\n"));
    }
    c.push_str(") {\n");
    for v in 0..d.vars {
        c.push_str(&format!(
            "    var v{v}: logic<16>;\n    assign o{v} = v{v};\n"
        ));
    }
    for a in 0..d.arrays {
        c.push_str(&format!("    var a{a}: logic<4> [4];\n    assign p{a} = {{a{a}[3], a{a}[2], a{a}[1], a{a}[0]}};\n"));
    }
    for &s in order {
        let g = &d.stmts[s];
        if g.len() == 1 && matches!(d.nodes[g[0]].body, Body::Plain(_)) {
            c.push_str(&format!(
                "    assign {}",
                node_text(&d.nodes[g[0]], "").trim_start()
            ));
        } else {
            c.push_str("    always_comb {\n");
            for &n in g {
                c.push_str(&node_text(&d.nodes[n], "        "));
            }
            c.push_str("    }\n");
        }
    }
    c.push_str("}\n");
    c
}

fn eval_src(s: Src, ins: &[u64], vars: &[u64], arr: &[[u64; 4]]) -> u64 {
    match s {
        Src::Input(k, lo, w) => (ins[k] >> lo) & ((1 << w) - 1),
        Src::Var(v, lo, w) => (vars[v] >> lo) & ((1 << w) - 1),
        Src::Elem(a, e) => arr[a][e],
    }
}

fn eval_expr(x: &Expr, w: usize, ins: &[u64], vars: &[u64], arr: &[[u64; 4]]) -> u64 {
    let a = eval_src(x.0, ins, vars, arr);
    let r = match &x.1 {
        None => a,
        Some((op, b)) => {
            let b = eval_src(*b, ins, vars, arr);
            match op {
                0 => a & b,
                1 => a | b,
                2 => a ^ b,
                _ => a + b,
            }
        }
    };
    r & ((1 << w) - 1)
}

fn reference(d: &Design, ins: &[u64]) -> (Vec<u64>, Vec<[u64; 4]>) {
    let mut vars = vec![0u64; d.vars];
    let mut arr = vec![[0u64; 4]; d.arrays];
    for n in &d.nodes {
        let w = width_of(n.target);
        let val = match &n.body {
            Body::Plain(x) => eval_expr(x, w, ins, &vars, &arr),
            Body::If((k, b), x, y) => {
                if (ins[*k] >> b) & 1 == 1 {
                    eval_expr(x, w, ins, &vars, &arr)
                } else {
                    eval_expr(y, w, ins, &vars, &arr)
                }
            }
            Body::Case(sel, arms, dflt) => {
                let s = eval_src(*sel, ins, &vars, &arr) as usize;
                if s < arms.len() {
                    eval_expr(&arms[s], w, ins, &vars, &arr)
                } else {
                    eval_expr(dflt, w, ins, &vars, &arr)
                }
            }
        };
        match n.target {
            Target::Var(v, nn, lo) => {
                let m = ((1u64 << (4 * nn)) - 1) << (4 * lo);
                vars[v] = (vars[v] & !m) | (val << (4 * lo));
            }
            Target::Elem(a, e) => arr[a][e] = val,
        }
    }
    (vars, arr)
}

/// Each design in two statement orders under each of `configs`: accepted, one
/// settle pass, and the reference model's values.
fn check_seeds(seeds: std::ops::Range<u64>, configs: &[Config]) {
    for seed in seeds {
        let design = generate(seed);
        let mut rng = Rng(seed + 3);
        for perm in 0..2 {
            let mut order: Vec<usize> = (0..design.stmts.len()).collect();
            if perm > 0 {
                rng.shuffle(&mut order);
            }
            let code = emit(&design, &order);
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
                    let inputs: Vec<u64> = (0..INPUTS).map(|_| rng.next() & 0xffff).collect();
                    for (k, &x) in inputs.iter().enumerate() {
                        sim.set(&format!("i{k}"), Value::new(x, 16, false));
                    }
                    sim.step(&Event::clock(VarId::SYNTHETIC));
                    let (vars, arrays) = reference(&design, &inputs);
                    let elements = arrays
                        .iter()
                        .map(|e| e[0] | (e[1] << 4) | (e[2] << 8) | (e[3] << 12));
                    let outputs = vars
                        .iter()
                        .enumerate()
                        .map(|(v, &w)| (format!("o{v}"), w))
                        .chain(elements.enumerate().map(|(a, w)| (format!("p{a}"), w)));
                    for (name, want) in outputs {
                        assert_eq!(
                            sim.get(&name).unwrap(),
                            Value::new(want, 16, false),
                            "{name}: seed {seed} order {perm} inputs {inputs:x?} {config:?}\n{code}"
                        );
                    }
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
    generated_wide_0: 0..48,
    generated_wide_1: 48..96,
    generated_wide_2: 96..144,
    generated_wide_3: 144..192,
}
