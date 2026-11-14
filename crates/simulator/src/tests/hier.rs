//! Hierarchy templates: a cell instantiated several times is scheduled once
//! and its instances run shared code.  Random cells are wired so that the
//! instance graph has cycles the statement graph does not; the result must
//! match the build without templates.

use super::*;

const IN: usize = 4;
const OUT: usize = 3;
const CELLS: usize = 4;
const TOP_IN: usize = 3;

/// `(op, lhs, rhs)`; operands index the inputs, then earlier temps.
type Term = (usize, usize, usize);

struct Cell {
    /// A second term writes the upper half from its own statement.
    temps: Vec<(Term, Option<Term>)>,
    /// Temp each output reads.
    outs: Vec<usize>,
}

impl Cell {
    fn operand(&self, k: usize) -> String {
        if k < IN {
            format!("a{k}")
        } else {
            format!("t{}", k - IN)
        }
    }

    /// Inputs each output depends on, as bit masks.
    fn deps(&self) -> Vec<u32> {
        let mut of: Vec<u32> = (0..IN).map(|k| 1 << k).collect();
        for &((_, l, r), hi) in &self.temps {
            let mut d = of[l] | of[r];
            if let Some((_, l2, r2)) = hi {
                d |= of[l2] | of[r2];
            }
            of.push(d);
        }
        self.outs.iter().map(|&t| of[IN + t]).collect()
    }
}

enum Source {
    Top(usize),
    Cell(usize, usize),
}

struct Design {
    cell: Cell,
    /// Per cell, per input.
    wiring: Vec<Vec<Source>>,
}

fn generate(seed: u64) -> Design {
    let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    let mut temps = Vec::new();
    for k in 0..6 + rng.below(6) {
        let term = (rng.below(4), rng.below(IN + k), rng.below(IN + k));
        let hi = rng
            .chance(30)
            .then(|| (rng.below(4), rng.below(IN + k), rng.below(IN + k)));
        temps.push((term, hi));
    }
    let outs = (0..OUT).map(|_| rng.below(temps.len())).collect();
    let cell = Cell { temps, outs };
    let deps = cell.deps();
    // Port graph: node `c * (IN + OUT) + p`, inputs first.
    let node = |c: usize, p: usize| c * (IN + OUT) + p;
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); CELLS * (IN + OUT)];
    for c in 0..CELLS {
        for (j, &d) in deps.iter().enumerate() {
            for i in 0..IN {
                if d & (1 << i) != 0 {
                    succ[node(c, i)].push(node(c, IN + j));
                }
            }
        }
    }
    let reaches = |succ: &Vec<Vec<usize>>, from: usize, to: usize| {
        let mut stack = vec![from];
        let mut seen = vec![false; succ.len()];
        while let Some(n) = stack.pop() {
            if n == to {
                return true;
            }
            if !std::mem::replace(&mut seen[n], true) {
                stack.extend(succ[n].iter().copied());
            }
        }
        false
    };
    let mut wiring: Vec<Vec<Source>> = (0..CELLS).map(|_| Vec::new()).collect();
    let mut slots: Vec<(usize, usize)> = (0..CELLS)
        .flat_map(|c| (0..IN).map(move |i| (c, i)))
        .collect();
    rng.shuffle(&mut slots);
    let mut chosen: Vec<Vec<Option<Source>>> = (0..CELLS)
        .map(|_| (0..IN).map(|_| None).collect())
        .collect();
    for (c, i) in slots {
        let mut src = Source::Top(rng.below(TOP_IN));
        if rng.chance(75) {
            for _ in 0..8 {
                let (k, j) = (rng.below(CELLS), rng.below(OUT));
                if !reaches(&succ, node(c, i), node(k, IN + j)) {
                    succ[node(k, IN + j)].push(node(c, i));
                    src = Source::Cell(k, j);
                    break;
                }
            }
        }
        chosen[c][i] = Some(src);
    }
    for (c, ins) in chosen.into_iter().enumerate() {
        wiring[c] = ins.into_iter().map(|s| s.unwrap()).collect();
    }
    Design { cell, wiring }
}

fn emit(d: &Design) -> String {
    let ops = ["+", "^", "&", "|"];
    let mut s = String::from("module Cell (\n    clk: input clock,\n    rst: input reset,\n");
    for i in 0..IN {
        s += &format!("    a{i}: input logic<8>,\n");
    }
    for j in 0..OUT {
        s += &format!("    y{j}: output logic<8>,\n");
    }
    s += "    r: output logic<8>,\n    z: output logic<8>,\n) {\n";
    for (k, &((op, l, r), hi)) in d.cell.temps.iter().enumerate() {
        s += &format!("    var t{k}: logic<8>;\n");
        match hi {
            None => {
                s += &format!(
                    "    assign t{k} = {} {} {};\n",
                    d.cell.operand(l),
                    ops[op],
                    d.cell.operand(r)
                );
            }
            Some((op2, l2, r2)) => {
                s += &format!(
                    "    assign t{k}[3:0] = {}[3:0] {} {}[3:0];\n",
                    d.cell.operand(l),
                    ops[op],
                    d.cell.operand(r)
                );
                s += &format!(
                    "    assign t{k}[7:4] = {}[7:4] {} {}[7:4];\n",
                    d.cell.operand(l2),
                    ops[op2],
                    d.cell.operand(r2)
                );
            }
        }
    }
    for (j, &t) in d.cell.outs.iter().enumerate() {
        s += &format!("    assign y{j} = t{t};\n");
    }
    // A version chain: `v` reads the first value of `u`.
    s += "    var u: logic<8>;\n    var v: logic<8>;\n    always_comb {\n        u = a0 + 8'd1;\n        v = u;\n        u = u ^ a1;\n    }\n    assign z = u + v;\n";
    s += "    var q: logic<8>;\n    always_ff {\n        if_reset { q = 0; }\n        else { q = q + y0; }\n    }\n    assign r = q;\n}\n";
    s += "module Top (\n    clk: input clock,\n    rst: input reset,\n";
    for i in 0..TOP_IN {
        s += &format!("    i{i}: input logic<8>,\n");
    }
    for c in 0..CELLS {
        for j in 0..OUT {
            s += &format!("    o{c}_{j}: output logic<8>,\n");
        }
        s += &format!("    r{c}: output logic<8>,\n    z{c}: output logic<8>,\n");
    }
    s += ") {\n";
    for c in 0..CELLS {
        for j in 0..OUT {
            s += &format!("    var w{c}_{j}: logic<8>;\n    assign o{c}_{j} = w{c}_{j};\n");
        }
    }
    for (c, ins) in d.wiring.iter().enumerate() {
        s += &format!("    inst c{c}: Cell (\n        clk,\n        rst,\n");
        for (i, src) in ins.iter().enumerate() {
            let text = match src {
                Source::Top(k) => format!("i{k}"),
                Source::Cell(k, j) => format!("w{k}_{j}"),
            };
            s += &format!("        a{i}: {text},\n");
        }
        for j in 0..OUT {
            s += &format!("        y{j}: w{c}_{j},\n");
        }
        s += &format!("        r: r{c},\n        z: z{c},\n    );\n");
    }
    s += "}\n";
    s
}

fn outputs(sim: &mut Simulator) -> Vec<Value> {
    let mut v = Vec::new();
    for c in 0..CELLS {
        for j in 0..OUT {
            v.push(sim.get(&format!("o{c}_{j}")).unwrap());
        }
        v.push(sim.get(&format!("r{c}")).unwrap());
        v.push(sim.get(&format!("z{c}")).unwrap());
    }
    v
}

fn check_seeds(seeds: std::ops::Range<u64>, use_4state: bool, chunk_mass: Option<usize>) {
    let flat = Config {
        use_jit: true,
        use_4state,
        ..Default::default()
    };
    check_seeds_with(seeds, flat, chunk_mass);
}

fn check_seeds_with(seeds: std::ops::Range<u64>, flat: Config, chunk_mass: Option<usize>) {
    crate::ir::hier::CHUNK_MASS.with(|m| m.set(chunk_mass));
    let mut engaged = 0;
    let n = seeds.end - seeds.start;
    for seed in seeds {
        let code = emit(&generate(seed));
        let hier = Config {
            hier_templates: Some(1),
            ..flat.clone()
        };
        let a = analyze_top(&code, &flat, "Top").unwrap();
        let b = match analyze_top(&code, &hier, "Top") {
            Ok(ir) => ir,
            Err(e) => panic!("seed {seed}: {e:?}\n{code}"),
        };
        assert_eq!(b.required_comb_passes, 1, "seed {seed}\n{code}");
        if b.hier_shared_blocks > 0 {
            engaged += 1;
            // A root of class calls must still land as compiled cells.
            if flat.aot_c {
                let root = b.dataflow.as_ref().and_then(|d| d.root_set());
                assert!(
                    root.is_some_and(|r| r.cell.get().is_some()),
                    "seed {seed}: no AOT-C root cells\n{code}"
                );
                assert!(
                    !b.hier_runs.is_empty() && b.hier_runs.values().all(|r| r.cells.aot_landed()),
                    "seed {seed}: no AOT-C template cells\n{code}"
                );
            }
        }
        let mut sa = Simulator::new(a, None);
        let mut sb = Simulator::new(b, None);
        let mut rng = Rng(seed + 1);
        for sim in [&mut sa, &mut sb] {
            let clk = sim.get_clock("clk").unwrap();
            let rst = sim.get_reset("rst").unwrap();
            sim.step_reset(&clk, &rst);
        }
        for step in 0..4 {
            let inputs: Vec<u64> = (0..TOP_IN).map(|_| rng.next() & 0xff).collect();
            for sim in [&mut sa, &mut sb] {
                for (k, &x) in inputs.iter().enumerate() {
                    sim.set(&format!("i{k}"), Value::new(x, 8, false));
                }
                let clk = sim.get_clock("clk").unwrap();
                sim.step(&clk);
            }
            assert_eq!(
                outputs(&mut sa),
                outputs(&mut sb),
                "seed {seed} step {step} inputs {inputs:x?}\n{code}"
            );
        }
    }
    assert!(
        engaged * 2 > n,
        "templates engaged in only {engaged} of {n} seeds"
    );
}

#[test]
fn hier_templates_match_flat_2state() {
    check_seeds(0..48, false, None);
}

#[test]
fn hier_templates_match_flat_4state() {
    check_seeds(48..80, true, None);
}

#[test]
fn hier_templates_match_flat_small_chunks() {
    check_seeds(80..128, false, Some(2));
}

/// The root's AOT-C cells run the template classes through a callback.
#[test]
fn hier_templates_match_flat_aot_c() {
    if !crate::tests::dataflow_on() || !crate::tests::aot_c_loads() {
        return;
    }
    let flat = Config {
        use_jit: true,
        aot_c: true,
        aot_c_async: false,
        aot_c_min_stmts: 0,
        ..Default::default()
    };
    check_seeds_with(128..136, flat, None);
}

/// The instances of a template call one shared AOT-C event body.
#[test]
fn hier_templates_match_flat_aot_c_events() {
    if !crate::tests::dataflow_on() || !crate::tests::aot_c_loads() {
        return;
    }
    let flat = Config {
        use_jit: true,
        aot_c: true,
        aot_c_event: true,
        aot_c_async: false,
        aot_c_min_stmts: 0,
        ..Default::default()
    };
    crate::backend::aot_c::emit::take_event_sources();
    check_seeds_with(136..152, flat, None);
    assert!(
        crate::backend::aot_c::emit::take_event_sources()
            .iter()
            .any(|s| s.contains("veryl_tev_")),
        "no event called a shared template body"
    );
}

/// A shared event body storing a wide value into comb storage marks its
/// readers when only bytes past the first one change.
#[test]
fn a_shared_event_body_marks_a_wide_store_changing_its_upper_bytes() {
    if !crate::tests::dataflow_on() || !crate::tests::aot_c_loads() {
        return;
    }
    let code = r#"
    module Sub (
        clk: input  clock     ,
        rst: input  reset     ,
        a  : input  logic<8>  ,
        q  : output logic<64> ,
        y  : output logic<8>  ,
    ) {
        var r: logic<64>;
        always_ff {
            if_reset { r = 0; } else { r = r + {a, 56'd0}; }
        }
        assign q = r;
        assign y = q[63:56] ^ a;
    }
    module Top (
        clk: input  clock   ,
        rst: input  reset   ,
        a  : input  logic<8>,
        y0 : output logic<8>,
        y1 : output logic<8>,
        y2 : output logic<8>,
        y3 : output logic<8>,
        z  : output logic<8>,
    ) {
        var q0: logic<64>;
        var q1: logic<64>;
        var q2: logic<64>;
        var q3: logic<64>;
        inst u0: Sub (clk, rst, a, q: q0, y: y0);
        inst u1: Sub (clk, rst, a, q: q1, y: y1);
        inst u2: Sub (clk, rst, a, q: q2, y: y2);
        inst u3: Sub (clk, rst, a, q: q3, y: y3);
        assign z = q0[63:56] + q1[63:56] + q2[63:56] + q3[63:56];
    }
    "#;
    let flat = Config {
        use_jit: true,
        aot_c: true,
        aot_c_event: true,
        aot_c_async: false,
        aot_c_min_stmts: 0,
        ..Default::default()
    };
    let hier = Config {
        hier_templates: Some(1),
        ..flat.clone()
    };
    crate::backend::aot_c::emit::take_event_sources();
    let mut sims: Vec<Simulator> = [&flat, &hier]
        .into_iter()
        .map(|c| Simulator::new(analyze_top(code, c, "Top").unwrap(), None))
        .collect();
    assert!(
        crate::backend::aot_c::emit::take_event_sources()
            .iter()
            .any(|s| s.contains("veryl_tev_")),
        "no event called a shared template body"
    );
    let ports = ["y0", "y1", "y2", "y3", "z"];
    for sim in sims.iter_mut() {
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step_reset(&clk, &rst);
    }
    for (step, a) in [1u64, 2, 3, 4, 5].into_iter().enumerate() {
        let mut got = Vec::new();
        for sim in sims.iter_mut() {
            sim.set("a", Value::new(a, 8, false));
            let clk = sim.get_clock("clk").unwrap();
            sim.step(&clk);
            got.push(ports.map(|p| sim.get(p).unwrap()));
        }
        assert_eq!(got[0], got[1], "step {step}");
    }
}

/// A parent of template classes rebuilt from the same inputs finds its
/// dataflow cells by those inputs: the class calls do not keep it from the
/// index.
/// Tests building in parallel share one conversion of a template instead of
/// each converting it.
#[test]
fn parallel_tests_share_one_template_conversion() {
    let config = Config {
        use_jit: true,
        dut_reuse: true,
        hier_templates: Some(1),
        ..Default::default()
    };
    let top: veryl_parser::resource_table::StrId = "Top".into();
    let mut checked = false;
    for seed in 0..8 {
        let code = emit(&generate(seed));
        let air_ir = analyze_air(&code);
        let session = crate::ir::BuildSession::new(&air_ir, &config, &[top]);
        let resources = veryl_parser::resource_table::export_tables();
        let texts = veryl_parser::text_table::export_tables();
        let engaged: Vec<bool> = std::thread::scope(|s| {
            let workers: Vec<_> = (0..2)
                .map(|_| {
                    let (resources, texts, session) = (&resources, &texts, &session);
                    std::thread::Builder::new()
                        .stack_size(crate::IR_WALK_STACK_BYTES)
                        .spawn_scoped(s, move || {
                            veryl_parser::resource_table::import_tables(resources);
                            veryl_parser::text_table::import_tables(texts);
                            crate::backend::inst::HIER_CLAIM_STALL
                                .with(|d| d.set(std::time::Duration::from_millis(200)));
                            session.build_ir(top).unwrap().hier_shared_blocks > 0
                        })
                        .unwrap()
                })
                .collect();
            workers.into_iter().map(|w| w.join().unwrap()).collect()
        });
        if !engaged.iter().all(|&e| e) {
            continue;
        }
        assert_eq!(session.hier_conversions(), 1, "seed {seed}\n{code}");
        checked = true;
        break;
    }
    assert!(checked, "no seed built a template");
}

#[test]
fn a_rebuilt_template_parent_loads_its_cells_without_emitting() {
    if !crate::tests::dataflow_on() || !crate::tests::aot_c_loads() {
        return;
    }
    let config = Config {
        use_jit: true,
        aot_c: true,
        aot_c_async: false,
        aot_c_min_stmts: 0,
        hier_templates: Some(1),
        ..Default::default()
    };
    crate::backend::aot_c::emit::set_test_aot_index(true);
    let mut checked = false;
    for seed in 128..136 {
        let code = emit(&generate(seed));
        crate::backend::aot_c::emit::take_dataflow_emits();
        let first = analyze_top(&code, &config, "Top").unwrap();
        let emitted = crate::backend::aot_c::emit::take_dataflow_emits();
        if first.hier_shared_blocks == 0 || emitted == 0 {
            continue;
        }
        crate::ir::opt::dataflow::cache_clear();
        let second = analyze_top(&code, &config, "Top").unwrap();
        assert_eq!(
            crate::backend::aot_c::emit::take_dataflow_emits(),
            0,
            "seed {seed}: the rebuild emitted its dataflow again\n{code}"
        );
        let root = second.dataflow.as_ref().and_then(|d| d.root_set());
        assert!(root.is_some_and(|r| r.cell.get().is_some()), "seed {seed}");
        checked = true;
        break;
    }
    crate::backend::aot_c::emit::set_test_aot_index(false);
    assert!(checked, "no seed built a parent of template classes");
}

/// A template's async reset, derived inside the parent, asserts with the
/// edge the parent takes, not one settle later through the port copy.
#[test]
fn hier_template_async_reset_from_parent_net() {
    let code = r#"
    module Sub (
        clk  : input  clock          ,
        rst_l: input  reset_async_low,
        q    : output logic          ,
    ) {
        var r: logic;
        always_ff (clk, rst_l) {
            if_reset { r = 1'b0; } else { r = 1'b1; }
        }
        assign q = r;
    }
    module SubH (
        clk  : input  clock           ,
        rst_h: input  reset_async_high,
        q    : output logic           ,
    ) {
        var r: logic;
        always_ff (clk, rst_h) {
            if_reset { r = 1'b0; } else { r = 1'b1; }
        }
        assign q = r;
    }
    module Top (
        clk : input  clock,
        trig: input  logic,
        q0  : output logic,
        q1  : output logic,
        q2  : output logic,
    ) {
        var f: logic;
        always_ff (clk) { f = trig; }
        let n    : logic               = ~f;
        let rst_a: '_ reset_async_low  = n;
        let rst_b: '_ reset_async_low  = n;
        inst u0: Sub (clk, rst_l: rst_a, q: q0);
        inst u1: Sub (clk, rst_l: rst_b, q: q1);
        let rst_h: '_ reset_async_high = ~rst_a;
        inst u2: SubH (clk, rst_h, q: q2);
    }
    "#;
    for hier in [None, Some(1)] {
        let config = Config {
            hier_templates: hier,
            ..Default::default()
        };
        let ir = analyze_top(code, &config, "Top").unwrap();
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        sim.set("trig", Value::new(0, 1, false));
        for _ in 0..4 {
            sim.step(&clk);
        }
        sim.set("trig", Value::new(1, 1, false));
        sim.step(&clk);
        for q in ["q0", "q1", "q2"] {
            assert_eq!(sim.get(q).unwrap(), Value::new(0, 1, false), "{q} {hier:?}");
        }
    }
}

/// A gated clock inside a template: the master's cone raises it outside the
/// template's cells, which must still bring it back down at the low phase
/// although the enable, a flop the cells read, no longer changes.
#[test]
fn hier_template_gated_clock_with_master_cone() {
    let mut fill = String::new();
    for i in 0..40 {
        let prev = if i == 0 {
            "q".to_string()
        } else {
            format!("t{}", i - 1)
        };
        fill += &format!("        var t{i}: logic<8>;\n        assign t{i} = {prev} ^ 8'd{i};\n");
    }
    let code = format!(
        r#"
    module Cell (
        clk: input  '_ clock,
        rst: input  '_ reset,
        o  : output logic<8>,
    ) {{
        var en: logic;
        always_ff (clk, rst) {{
            if_reset {{ en = 0; }} else {{ en = 1; }}
        }}
        let gclk: '_ clock = clk & en;
        var q: logic<8>;
        always_ff (gclk, rst) {{
            if_reset {{ q = 0; }} else {{ q += 1; }}
        }}
{fill}        assign o = t39;
    }}
    module Top (
        clk: input  '_ clock,
        rst: input  '_ reset,
        o0 : output logic<8>,
        o1 : output logic<8>,
    ) {{
        inst c0: Cell (clk, rst, o: o0);
        inst c1: Cell (clk, rst, o: o1);
    }}
"#
    );
    let run = |hier: Option<usize>| {
        let config = Config {
            use_jit: true,
            hier_templates: hier,
            ..Default::default()
        };
        let ir = analyze_top(&code, &config, "Top").unwrap();
        let cells = ir.hier_shared_blocks;
        // The root's dataflow cells take the cone's place: the clock marks
        // its readers.
        let root_cells = ir.dataflow.as_ref().is_some_and(|d| d.root_set().is_some());
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step_reset(&clk, &rst);
        let mut seen = Vec::new();
        for _ in 0..12 {
            sim.step(&clk);
            seen.push((sim.get("o0").unwrap(), sim.get("o1").unwrap()));
        }
        (seen, cells, sim.master_cone_settles > 0 || root_cells)
    };
    let (flat, _, _) = run(None);
    let (hier, cells, partial) = run(Some(1));
    assert!(
        cells > 0 && partial,
        "cells {cells} partial settle {partial}"
    );
    assert_eq!(flat, hier);
}

/// A template's comb reads its flops: every commit that changes one must
/// rerun the cells reading it, though nothing compares the flops.
#[test]
fn hier_template_cells_follow_their_flops() {
    let code = r#"
    module Acc (
        clk: input  clock   ,
        rst: input  reset   ,
        a  : input  logic<8>,
        o  : output logic<8>,
    ) {
        var r: logic<8>;
        var s: logic<8>;
        always_ff (clk, rst) {
            if_reset { r = 0; s = 1; } else { r += a; s = s + r; }
        }
        let x: logic<8> = r ^ 8'h5a;
        let y: logic<8> = x + s;
        assign o = y ^ {r[3:0], s[7:4]};
    }
    module Top (
        clk: input  clock   ,
        rst: input  reset   ,
        a  : input  logic<8>,
        o0 : output logic<8>,
        o1 : output logic<8>,
    ) {
        let b: logic<8> = a + 8'd3;
        inst u0: Acc (clk, rst, a, o: o0);
        inst u1: Acc (clk, rst, a: b, o: o1);
    }
    "#;
    let run = |hier: Option<usize>| {
        let config = Config {
            use_jit: true,
            hier_templates: hier,
            ..Default::default()
        };
        let ir = analyze_top(code, &config, "Top").unwrap();
        let cells = ir.hier_shared_blocks;
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step_reset(&clk, &rst);
        let mut seen = Vec::new();
        for i in 0..24u64 {
            // Inputs that hold still for a few cycles leave the flops as the
            // only thing changing.
            sim.set("a", Value::new(i / 4, 8, false));
            sim.step(&clk);
            seen.push((sim.get("o0").unwrap(), sim.get("o1").unwrap()));
        }
        (seen, cells)
    };
    let (flat, _) = run(None);
    let (hier, cells) = run(Some(1));
    assert!(cells > 0, "no template engaged");
    assert_eq!(flat, hier);
}

/// A template whose levels do not settle falls back, and the top then builds
/// exactly as it would without templates.
#[test]
fn a_fallen_back_template_builds_as_flat() {
    let n = 70;
    let mut code = String::from(
        "module Sub (a: input logic<8>, x: output logic<8>) { assign x = a + 8'd1; }\n\
         module Top (i: input logic<8>, o: output logic<8>) {\n",
    );
    for k in 0..=n {
        code.push_str(&format!("    var w{k}: logic<8>;\n"));
    }
    code.push_str("    assign w0 = i;\n");
    for k in 0..n {
        code.push_str(&format!("    inst s{k}: Sub (a: w{k}, x: w{});\n", k + 1));
    }
    code.push_str(&format!("    assign o = w{n};\n}}\n"));
    let top: veryl_parser::resource_table::StrId = "Top".into();
    let air_ir = analyze_air(&code);
    let build = |hier| {
        let config = Config {
            hier_templates: hier,
            ..Default::default()
        };
        crate::ir::BuildSession::new(&air_ir, &config, &[top])
            .build_ir(top)
            .unwrap()
    };
    let (flat, hier) = (build(None), build(Some(1)));
    assert_eq!(hier.hier_shared_blocks, 0, "the template did not fall back");
    assert_eq!(hier.comb_statements.len(), flat.comb_statements.len());
    assert_eq!(hier.comb_values.len(), flat.comb_values.len());
    assert_eq!(hier.ff_values.len(), flat.ff_values.len());
    let mut sim = Simulator::new(hier, None);
    sim.set("i", Value::new(5, 8, false));
    assert_eq!(sim.get("o").unwrap(), Value::new(75, 8, false));
}

/// `[test] shared_modules` makes exactly the named modules templates, however
/// small they are.
#[test]
fn a_named_module_becomes_a_template() {
    let code = r#"
    module Inc (a: input logic<8>, x: output logic<8>) { assign x = a + 8'd1; }
    module Dec (a: input logic<8>, x: output logic<8>) { assign x = a - 8'd1; }
    module Top (i: input logic<8>, o: output logic<8>, p: output logic<8>) {
        var w: logic<8>;
        var v: logic<8>;
        inst i0: Inc (a: i, x: w);
        inst i1: Inc (a: w, x: o);
        inst d0: Dec (a: i, x: v);
        inst d1: Dec (a: v, x: p);
    }
    "#;
    let top: veryl_parser::resource_table::StrId = "Top".into();
    let air_ir = analyze_air(code);
    let build = |named: &[&str]| {
        let config = Config {
            use_jit: true,
            shared_modules: named.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        crate::ir::BuildSession::new(&air_ir, &config, &[top])
            .build_ir(top)
            .unwrap()
    };
    assert_eq!(build(&[]).hier_shared_blocks, 0);
    let inc = build(&["Inc"]);
    let both = build(&["Inc", "Dec"]);
    assert!(inc.hier_shared_blocks > 0);
    assert!(both.hier_shared_blocks > inc.hier_shared_blocks);
    let mut sim = Simulator::new(both, None);
    sim.set("i", Value::new(5, 8, false));
    assert_eq!(sim.get("o").unwrap(), Value::new(7, 8, false));
    assert_eq!(sim.get("p").unwrap(), Value::new(3, 8, false));
}

/// Builds `code` without templates and with `named` shared, steps both
/// through `inputs` on `a`, and returns the second build's session counters
/// `(conversions, matched)` once every port in `ports` agreed.
fn compare_named(
    code: &str,
    flat: Config,
    named: &[&str],
    ports: &[&str],
    inputs: &[u64],
) -> (usize, usize) {
    let top: veryl_parser::resource_table::StrId = "Top".into();
    let air_ir = analyze_air(code);
    let shared = Config {
        shared_modules: named.iter().map(|s| s.to_string()).collect(),
        ..flat.clone()
    };
    let session = crate::ir::BuildSession::new(&air_ir, &shared, &[top]);
    let mut sims = [
        Simulator::new(
            crate::ir::BuildSession::new(&air_ir, &flat, &[top])
                .build_ir(top)
                .unwrap(),
            None,
        ),
        Simulator::new(session.build_ir(top).unwrap(), None),
    ];
    for sim in sims.iter_mut() {
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step_reset(&clk, &rst);
    }
    for (step, &a) in inputs.iter().enumerate() {
        let mut got = Vec::new();
        for sim in sims.iter_mut() {
            sim.set("a", Value::new(a, 8, false));
            let clk = sim.get_clock("clk").unwrap();
            sim.step(&clk);
            got.push(
                ports
                    .iter()
                    .map(|p| sim.get(p).unwrap())
                    .collect::<Vec<_>>(),
            );
        }
        assert_eq!(got[0], got[1], "step {step}");
    }
    (session.hier_conversions(), session.hier_matched())
}

/// A template whose hazard block sits on a false cycle orders in one pass by
/// splitting only that block, like the flat build.
#[test]
fn a_template_splits_its_block_on_a_false_cycle() {
    let code = r#"
    module Rf (
        clk : input  clock       ,
        rst : input  reset       ,
        i_wa: input  logic<2>    ,
        i_wd: input  logic<8>    ,
        i_ra: input  logic<2> [2],
        o_rd: output logic<8> [2],
    ) {
        var mem: logic<8> [4];
        always_ff {
            if_reset {
                mem = '{default: 0};
            } else {
                mem[i_wa] = i_wd;
            }
        }
        for r in 0..2 :g {
            assign o_rd[r] = mem[i_ra[r]];
        }
    }
    module Lane (
        clk: input  clock   ,
        rst: input  reset   ,
        a  : input  logic<8>,
        q  : output logic<8>,
    ) {
        var sel: logic<3> [2];
        var c  : logic<2> [2];
        var x  : logic<8> [2];
        var o  : logic<8> [2];
        var p  : logic<8> [2];
        var ra : logic<2> [2];
        var rd : logic<8> [2];
        assign sel = '{a[4:2], a[7:5]};
        assign c   = '{a[1:0], a[3:2]};
        assign x   = '{a, ~a};
        assign q   = o[0] ^ o[1] ^ p[0] ^ p[1];

        always_comb {
            for l in 0..2 {
                var k: logic<3>;
                k     = sel[l];
                ra[l] = k[1:0];
                o[l]  = if k[2] ? rd[l] : 8'd0;
            }
        }
        inst u: Rf (
            clk         ,
            rst         ,
            i_wa: a[1:0],
            i_wd: a     ,
            i_ra: ra    ,
            o_rd: rd    ,
        );
        always_comb {
            for l in 0..2 {
                var t: logic<8>;
                case c[l] {
                    2'd3   : t = x[l];
                    default: {}
                }
                case c[l] {
                    2'd3   : p[l] = t;
                    2'd2   : p[l] = 8'h01;
                    default: p[l] = 8'h00;
                }
            }
        }
    }
    module Top (
        clk: input  clock   ,
        rst: input  reset   ,
        a  : input  logic<8>,
        q0 : output logic<8>,
        q1 : output logic<8>,
    ) {
        var na: logic<8>;
        assign na = ~a;
        inst l0: Lane (clk, rst, a, q: q0);
        inst l1: Lane (clk, rst, a: na, q: q1);
    }
    "#;
    let flat = Config {
        use_jit: true,
        ..Default::default()
    };
    let inputs = [0x5f, 0xa7, 0x3c, 0xff, 0x81, 0x24, 0xdb, 0x66];
    let (conversions, _) = compare_named(code, flat, &["Lane"], &["q0", "q1"], &inputs);
    assert!(conversions > 0, "not a template");
}

/// Specializations of a named module that differ only in a parameter read as
/// a value, here through a child of their own, run one template.
#[test]
fn specializations_differing_in_a_value_parameter_share_a_template() {
    let code = r#"
    module Hit #(param P: u32 = 0) (a: input logic<8>, x: output logic) {
        assign x = a[1:0] == P;
    }
    module Port #(param P: u32 = 0) (
        clk: input  clock   ,
        rst: input  reset   ,
        a  : input  logic<8>,
        q  : output logic<8>,
    ) {
        var hit: logic;
        var r  : logic<8>;
        inst u_hit: Hit #(P) (a, x: hit);
        always_ff {
            if_reset { r = 0; } else if hit { r = r + a; }
        }
        assign q = r;
    }
    module Top (
        clk: input  clock     ,
        rst: input  reset     ,
        a  : input  logic<8>  ,
        q  : output logic<8>[4],
    ) {
        for i in 0..4 :g {
            inst u: Port #(P: i) (clk, rst, a, q: q[i]);
        }
    }
    "#;
    let inputs = [0, 1, 2, 3, 5, 6, 7, 9];
    let jit = Config {
        use_jit: true,
        ..Default::default()
    };
    assert_eq!(compare_named(code, jit, &["Port"], &["q"], &inputs), (1, 3));
    if crate::tests::aot_c_loads() {
        let aot = Config {
            use_jit: true,
            aot_c: true,
            aot_c_event: true,
            aot_c_async: false,
            aot_c_min_stmts: 0,
            ..Default::default()
        };
        assert_eq!(compare_named(code, aot, &["Port"], &["q"], &inputs), (1, 3));
    }
}

/// A parameter that shapes the code keeps each specialization to itself,
/// with the same results.
#[test]
fn specializations_differing_in_shape_convert_flat() {
    let code = r#"
    module Port #(param P: u32 = 0) (
        clk: input  clock   ,
        rst: input  reset   ,
        a  : input  logic<8>,
        q  : output logic<8>,
    ) {
        var r: logic<8>;
        if P == 0 :g {
            always_ff { if_reset { r = 0; } else { r = r + a; } }
        } else {
            always_ff { if_reset { r = 0; } else { r = r ^ a; } }
        }
        assign q = r;
    }
    module Top (
        clk: input  clock     ,
        rst: input  reset     ,
        a  : input  logic<8>  ,
        q  : output logic<8>[4],
    ) {
        for i in 0..4 :g {
            inst u: Port #(P: i % 2) (clk, rst, a, q: q[i]);
        }
    }
    "#;
    let jit = Config {
        use_jit: true,
        ..Default::default()
    };
    let (_, matched) = compare_named(code, jit, &["Port"], &["q"], &[1, 2, 3, 4]);
    assert_eq!(matched, 0);
}

/// Two templates each have an instance at the same relative offset: their
/// class runs and partitions must not be taken for each other.
#[test]
fn two_templates_keep_their_own_runs() {
    let code = r#"
    module Inc (a: input logic<8>, x: output logic<8>) { assign x = a + 8'd1; }
    module Mix (a: input logic<8>, b: input logic<8>, x: output logic<8>, y: output logic<8>) {
        var t: logic<8>;
        assign t = a ^ b;
        assign x = t + 8'd3;
        assign y = t & a;
    }
    module Top (i: input logic<8>, j: input logic<8>, o: output logic<8>, p: output logic<8>, q: output logic<8>) {
        var w: logic<8>;
        var v: logic<8>;
        var u: logic<8>;
        inst i0: Inc (a: i, x: w);
        inst i1: Inc (a: w, x: o);
        inst m0: Mix (a: i, b: j, x: v, y: u);
        inst m1: Mix (a: v, b: u, x: p, y: q);
    }
    "#;
    let top: veryl_parser::resource_table::StrId = "Top".into();
    let air_ir = analyze_air(code);
    let mut configs = vec![Config {
        use_jit: true,
        ..Default::default()
    }];
    if crate::tests::aot_c_loads() {
        configs.push(Config {
            use_jit: true,
            aot_c: true,
            aot_c_async: false,
            aot_c_min_stmts: 0,
            ..Default::default()
        });
    }
    for base in configs {
        let build = |named: &[&str]| {
            let config = Config {
                shared_modules: named.iter().map(|s| s.to_string()).collect(),
                ..base.clone()
            };
            crate::ir::BuildSession::new(&air_ir, &config, &[top])
                .build_ir(top)
                .unwrap()
        };
        let shared = build(&["Inc", "Mix"]);
        assert!(shared.hier_shared_blocks > 0);
        let mut flat = Simulator::new(build(&[]), None);
        let mut sim = Simulator::new(shared, None);
        let mut rng = Rng(7);
        for step in 0..16 {
            let (i, j) = (rng.next() & 0xff, rng.next() & 0xff);
            for s in [&mut flat, &mut sim] {
                s.set("i", Value::new(i, 8, false));
                s.set("j", Value::new(j, 8, false));
                s.step(&Event::clock(VarId::SYNTHETIC));
            }
            for name in ["o", "p", "q"] {
                assert_eq!(sim.get(name), flat.get(name), "{name} step {step} {base:?}");
            }
        }
    }
}

/// A named module instantiated under a generate loop and an `if` generate,
/// inside an instance with overridden parameters, still becomes a template.
#[test]
fn a_named_template_under_generate_and_a_param_override() {
    let code = r#"
    module Sub (a: input logic<8>, x: output logic<8>) { assign x = a + 8'd1; }
    module Empty (a: input logic<8>, x: output logic<8>) { assign x = a; }
    module Mid #(param P: u32 = 0) (i: input logic<8>, o: output logic<8>) {
        var w: logic<8> [3];
        assign w[0] = i;
        for k in 0..2 :g {
            if P == 0 :h {
                inst s: Sub (a: w[k], x: w[k + 1]);
            } else {
                inst e: Empty (a: w[k], x: w[k + 1]);
            }
        }
        assign o = w[2];
    }
    module Top (i: input logic<8>, o: output logic<8>) {
        inst u: Mid #(P: 0) (i, o);
    }
    "#;
    let top: veryl_parser::resource_table::StrId = "Top".into();
    let air_ir = analyze_air(code);
    let config = Config {
        use_jit: true,
        shared_modules: vec!["Sub".to_string()],
        ..Default::default()
    };
    let ir = crate::ir::BuildSession::new(&air_ir, &config, &[top])
        .build_ir(top)
        .unwrap();
    assert!(ir.hier_shared_blocks > 0, "not a template");
}

/// One specialization several tops instantiate is not a group of them.
#[test]
fn a_specialization_reached_from_several_tops_is_one() {
    let code = r#"
    module Sub (a: input logic<8>, x: output logic<8>) { assign x = a + 8'd1; }
    module TopA (i: input logic<8>, o: output logic<8>) {
        var w: logic<8>;
        inst s0: Sub (a: i, x: w);
        inst s1: Sub (a: w, x: o);
    }
    module TopB (i: input logic<8>, o: output logic<8>) {
        var w: logic<8>;
        inst s0: Sub (a: i, x: w);
        inst s1: Sub (a: w, x: o);
    }
    "#;
    let air_ir = analyze_air(code);
    let tops: Vec<veryl_parser::resource_table::StrId> = vec!["TopA".into(), "TopB".into()];
    let set = crate::ir::hier::template_set(&air_ir, &tops, None, &["Sub".to_string()]);
    assert_eq!(set.templates.len(), 1);
    assert!(set.groups.is_empty() && set.lifted.is_empty());
}
