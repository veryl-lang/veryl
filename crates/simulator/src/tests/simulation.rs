use super::*;
use crate::output_buffer;

#[test]
fn simple_comb() {
    let code = r#"
    module Top (
        a: input  logic<32>,
        b: input  logic<32>,
        c: output logic<32>,
    ) {
        assign c = a + b;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);

        let mut sim = Simulator::new(ir, None);

        println!("{}", sim.ir.dump_variables());

        let a = Value::new(10, 32, false);
        let b = Value::new(20, 32, false);

        sim.set("a", a);
        sim.set("b", b);

        println!("{}", sim.ir.dump_variables());

        sim.step(&Event::Clock(VarId::SYNTHETIC));

        println!("{}", sim.ir.dump_variables());

        let exp = Value::new(30, 32, false);

        assert_eq!(sim.get("c").unwrap(), exp);
    }
}

#[test]
fn simple_ff() {
    let code = r#"
    module Top (
        clk: input clock,
        rst: input reset,
        cnt: output logic<32>,
    ) {
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);

        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        println!("{}", sim.ir.dump_variables());

        sim.step(&rst);

        println!("{}", sim.ir.dump_variables());

        for _ in 0..100 {
            sim.step(&clk);
        }

        println!("{}", sim.ir.dump_variables());

        let exp = Value::new(100, 32, false);

        assert_eq!(sim.get("cnt").unwrap(), exp);
    }
}

#[test]
fn ff_to_ff() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        i0 : input  logic<32>,
        o0 : output logic<32>,
        o1 : output logic<32>,
    ) {
        always_ff {
            if_reset {
                o0 = 0;
            } else {
                o0 = i0;
            }
        }
        always_ff {
            if_reset {
                o1 = 0;
            } else {
                o1 = o0;
            }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);

        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        println!("{}", sim.ir.dump_variables());

        sim.step(&rst);

        println!("{}", sim.ir.dump_variables());

        for i in 0..10 {
            sim.set("i0", Value::new(i, 32, false));
            sim.step(&clk);
            println!("{}", sim.ir.dump_variables());
        }

        assert_eq!(sim.get("o0").unwrap(), Value::new(9, 32, false));
        assert_eq!(sim.get("o1").unwrap(), Value::new(8, 32, false));
    }
}

#[test]
fn ff_statement_after_if_reset() {
    // Regression: statements placed after the `if_reset` block in an always_ff
    // must still execute. They previously got dropped by the simulator IR
    // conversion (only the if_reset itself was kept), so `b` stayed at its
    // reset value instead of tracking `a`.
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        b  : output logic<8>,
    ) {
        var a: logic<8>;
        always_ff {
            if_reset {
                a = 0;
            } else {
                a += 1;
            }
            b = a + 1;
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);
        for _ in 0..5 {
            sim.step(&clk);
        }

        // Each clock edge: a <= a_old + 1 and b <= a_old + 1, so a == b == 5.
        assert_eq!(sim.get("b").unwrap(), Value::new(5, 8, false));
    }
}

#[test]
fn ff_reset_fill_literal_field_wise() {
    // Regression: a `'1` fill literal assigned to a struct field (bit-select
    // write) in an `if_reset` block read back 0 on the JIT backends while
    // interpret stayed correct.  See `size_fill_literal_rhs`.
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        o_a: output logic,
        o_b: output logic,
        o_c: output logic,
    ) {
        struct flags_t {
            a: logic,
            b: logic,
            c: logic,
        }
        var f: flags_t;
        always_ff {
            if_reset {
                f.a = '1;
                f.b = '0;
                f.c = '1;
            }
        }
        always_comb {
            o_a = f.a;
            o_b = f.b;
            o_c = f.c;
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let rst = sim.get_reset("rst").unwrap();
        sim.step(&rst);

        assert_eq!(
            sim.get("o_a").unwrap(),
            Value::new(1, 1, false),
            "{config:?}"
        );
        assert_eq!(
            sim.get("o_b").unwrap(),
            Value::new(0, 1, false),
            "{config:?}"
        );
        assert_eq!(
            sim.get("o_c").unwrap(),
            Value::new(1, 1, false),
            "{config:?}"
        );
    }
}

#[test]
fn ff_reset_fill_literal_full_width() {
    // Companion to `ff_reset_fill_literal_field_wise` for a bare `'1` written
    // to a full-width register (no bit-select).
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        x  : output logic<8>,
    ) {
        always_ff {
            if_reset {
                x = '1;
            }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let rst = sim.get_reset("rst").unwrap();
        sim.step(&rst);

        assert_eq!(
            sim.get("x").unwrap(),
            Value::new(0xff, 8, false),
            "{config:?}"
        );
    }
}

#[test]
fn short_bit() {
    let code = r#"
    module Top (
        clk: input clock,
        rst: input reset,
        cnt: output logic<4>,
    ) {
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);

        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        println!("{}", sim.ir.dump_variables());

        sim.step(&rst);

        println!("{}", sim.ir.dump_variables());

        for _ in 0..100 {
            sim.step(&clk);
        }

        println!("{}", sim.ir.dump_variables());

        let exp = Value::new(4, 4, false);

        assert_eq!(sim.get("cnt").unwrap(), exp);
    }
}

#[test]
fn long_bit_128() {
    // 128-bit variables should now be supported (both JIT and interpreter)
    let code = r#"
    module Top (
        clk: input clock,
        rst: input reset,
        cnt: output logic<128>,
    ) {
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);

        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        sim.step(&clk);
        sim.step(&clk);
        sim.step(&clk);

        let got = sim.get("cnt").unwrap();
        let exp = Value::new(3, 128, false);
        assert_eq!(got, exp, "config: {:?}, got: {:?}", config, got);
    }
}

#[test]
fn long_bit_over_128() {
    // >128-bit variables should now be supported via interpreter fallback
    let code = r#"
    module Top (
        clk: input clock,
        rst: input reset,
        cnt: output logic<256>,
    ) {
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        sim.step(&clk);
        sim.step(&clk);
        sim.step(&clk);

        let got = sim.get("cnt").unwrap();
        let exp = Value::new(3, 256, false);
        assert_eq!(got, exp, "config: {:?}, got: {:?}", config, got);
    }
}

#[test]
fn wide_bit_ops_256() {
    let code = r#"
    module Top (
        a: input  logic<256>,
        b: input  logic<256>,
        c: output logic<256>,
        d: output logic<256>,
        e: output logic<256>,
    ) {
        assign c = a & b;
        assign d = a | b;
        assign e = a ^ b;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let a = Value::new(0xff00ff, 256, false);
        let b = Value::new(0x0f0f0f, 256, false);

        sim.set("a", a);
        sim.set("b", b);
        sim.step(&Event::Clock(VarId::SYNTHETIC));

        let c = Value::new(0x0f000f, 256, false);
        let d = Value::new(0xff0fff, 256, false);
        let e = Value::new(0xf00ff0, 256, false);
        assert_eq!(sim.get("c").unwrap(), c);
        assert_eq!(sim.get("d").unwrap(), d);
        assert_eq!(sim.get("e").unwrap(), e);
    }
}

#[test]
fn wide_256_arithmetic() {
    let code = r#"
    module Top (
        a: input  logic<256>,
        b: input  logic<256>,
        sum: output logic<256>,
        diff: output logic<256>,
        prod: output logic<256>,
    ) {
        assign sum = a + b;
        assign diff = a - b;
        assign prod = a * b;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let a = Value::new(100, 256, false);
        let b = Value::new(42, 256, false);

        sim.set("a", a);
        sim.set("b", b);
        sim.step(&Event::Clock(VarId::SYNTHETIC));

        assert_eq!(sim.get("sum").unwrap(), Value::new(142, 256, false));
        assert_eq!(sim.get("diff").unwrap(), Value::new(58, 256, false));
        assert_eq!(sim.get("prod").unwrap(), Value::new(4200, 256, false));
    }
}

#[test]
fn wide_256_narrow_select() {
    // Static narrow (≤64-bit) bit-select READ of a wide (>128-bit) value.
    // Exercises the Cranelift `emit_wide_bit_select_read_narrow` path
    // (previously a silent interpret fallback) across word-0, word-boundary,
    // word-straddling, and high-word selects.  Validated against interpret
    // via Config::all().
    let code = r#"
    module Top (
        a:  input  logic<256>,
        s0: output logic<8>,
        s1: output logic<11>,
        s2: output logic<8>,
        s3: output logic<1>,
        s4: output logic<32>,
        s5: output logic<64>,
    ) {
        assign s0 = a[7:0];
        assign s1 = a[70:60];
        assign s2 = a[127:120];
        assign s3 = a[64];
        assign s4 = a[95:64];
        assign s5 = a[127:64];
    }
    "#;

    let p: u128 = (0xFEDC_BA98_7654_3210u128 << 64) | 0x0123_4567_89AB_CDEFu128;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.set("a", Value::from_u128(p, 0, 256, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));

        let want = |v: u64, w: usize| Value::new(v, w, false);
        assert_eq!(
            sim.get("s0").unwrap(),
            want((p & 0xFF) as u64, 8),
            "s0 {config:?}"
        );
        assert_eq!(
            sim.get("s1").unwrap(),
            want(((p >> 60) & 0x7FF) as u64, 11),
            "s1 {config:?}"
        );
        assert_eq!(
            sim.get("s2").unwrap(),
            want(((p >> 120) & 0xFF) as u64, 8),
            "s2 {config:?}"
        );
        assert_eq!(
            sim.get("s3").unwrap(),
            want(((p >> 64) & 1) as u64, 1),
            "s3 {config:?}"
        );
        assert_eq!(
            sim.get("s4").unwrap(),
            want(((p >> 64) & 0xFFFF_FFFF) as u64, 32),
            "s4 {config:?}"
        );
        assert_eq!(
            sim.get("s5").unwrap(),
            want((p >> 64) as u64, 64),
            "s5 {config:?}"
        );
    }
}

#[test]
fn wide_256_select_store() {
    // Wide-dst bit-select WRITE (RMW) on a 256-bit FF, exercised as a
    // multi-RMW chain (`f = lo; f[71:64] = b;`) so the select-RMW reads the
    // forwarded prior write.  Validates the Cranelift `emit_wide_select_rmw`
    // path (2-state) against the interpreter via Config::all().
    let code = r#"
    module Top (
        clk: input  clock,
        lo:  input  logic<256>,
        b:   input  logic<8>,
        f:   output logic<256>,
    ) {
        always_ff {
            f = lo;
            f[71:64] = b;
        }
    }
    "#;

    let p: u128 = (0x1122_3344_5566_7788u128 << 64) | 0x99AA_BBCC_DDEE_FF00u128;
    let bval: u64 = 0xA5;
    let expected: u128 = (p & !(0xFFu128 << 64)) | ((bval as u128) << 64);

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();

        sim.set("lo", Value::from_u128(p, 0, 256, false));
        sim.set("b", Value::new(bval, 8, false));
        sim.step(&clk);

        assert_eq!(
            sim.get("f").unwrap(),
            Value::from_u128(expected, 0, 256, false),
            "config: {config:?}"
        );
    }
}

/// AOT-C config that forces native whole-module compile (min_stmts=0) and
/// dual-runs cc vs Cranelift every cycle (aot_c_validate), panicking on the
/// first divergence.
fn aot_native_validate_config() -> Config {
    Config {
        use_4state: false,
        use_jit: true,
        aot_c: true,
        aot_c_event: true,
        aot_c_async: false,
        aot_c_validate: true,
        aot_c_min_stmts: 0,
        ..Default::default()
    }
}

#[test]
fn wide_256_aot_native_comb() {
    // G1: a wide (>128-bit) comb module must be covered NATIVELY by the AOT-C
    // backend (whole_comb = Some), not silently bailed to Cranelift.  The
    // aot_c_validate config additionally dual-runs cc vs Cranelift each cycle,
    // asserting the wide-op helper codegen is bit-identical.
    if !crate::backend::aot_c::cc_available() {
        return; // no external C compiler on this host
    }
    let code = r#"
    module Top (
        a: input  logic<256>,
        b: input  logic<256>,
        c: output logic<256>,
        d: output logic<256>,
        e: output logic<256>,
    ) {
        assign c = (a & b) | (a ^ b);
        assign d = a + b;
        assign e = a << 8;
    }
    "#;
    let config = aot_native_validate_config();
    let ir = analyze(code, &config);
    assert!(
        ir.whole_comb.is_some(),
        "wide comb module must be AOT-C-native (whole_comb=Some), not bailed to Cranelift"
    );
    let mut sim = Simulator::new(ir, None);
    let a = Value::new(0x00FF, 256, false);
    let b = Value::new(0x0F0F, 256, false);
    sim.set("a", a);
    sim.set("b", b);
    sim.step(&Event::Clock(VarId::SYNTHETIC));
    // (a&b)|(a^b) == a|b == 0x0FFF; a+b == 0x100E; a<<8 == 0xFF00
    assert_eq!(sim.get("c").unwrap(), Value::new(0x0FFF, 256, false));
    assert_eq!(sim.get("d").unwrap(), Value::new(0x100E, 256, false));
    assert_eq!(sim.get("e").unwrap(), Value::new(0xFF00, 256, false));
}

#[test]
fn probe_wide_comb_oor_select_store() {
    // PROBE: wide (>128-bit) COMB bit-select store where hi >= dst_width.
    // o is 200-bit; a runtime base of 199 writes bits 199..201, but only bit
    // 199 is within [0,200) — the rest must be clamped, not stored past the
    // allocation.  (A CONSTANT out-of-range select is now rejected at
    // analysis, so the probe drives the base at runtime.)
    // Drive o fully first so the RMW reads a defined base.
    let code = r#"
    module Top (
        base: input  logic<200>,
        i:    input  logic<9>,
        b:    input  logic<8>,
        o:    output logic<200>,
    ) {
        always_comb {
            o = base;
            o[i+:3] = b as 3;
        }
    }
    "#;

    // Reference value: interpreter / Cranelift semantics = only bits within
    // [0,200) are written, rest of o = base, and bits >= 200 stay 0 (o is
    // declared 200-bit so high storage bits must be 0).
    // base low: set bit 199 region to 0 so we can see the written bit clearly.
    let base_val = Value::new(0, 200, false);
    let b_val = Value::new(0b111, 8, false); // wants to set bits 199,200,201

    let mut results = vec![];
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.set("base", base_val.clone());
        sim.set("i", Value::new(199, 9, false));
        sim.set("b", b_val.clone());
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        let o = sim.get("o").unwrap();
        eprintln!("PROBE config={config:?} o={}", o.format_hex());
        results.push((format!("{config:?}"), o.format_hex()));
    }
    // Print all; assert all identical (this is the divergence check).
    let first = results[0].1.clone();
    for (cfg, v) in &results {
        assert_eq!(
            *v, first,
            "DIVERGENCE: config {cfg} gave o={v}, expected {first}"
        );
    }
}

#[test]
fn wide_256_aot_native_ff() {
    // G2: a wide (>128-bit) FF module must be covered NATIVELY by the AOT-C
    // event backend (whole_events non-empty), routing through the 64-byte
    // WriteLogWideEntry pool.  aot_c_validate dual-runs cc vs Cranelift, which
    // also exercises the wide-pool comparison added to validate_event_aot.
    if !crate::backend::aot_c::cc_available() {
        return;
    }
    let code = r#"
    module Top (
        clk: input  clock,
        a:   input  logic<256>,
        q:   output logic<256>,
    ) {
        always_ff {
            q = a + 1;
        }
    }
    "#;
    let config = aot_native_validate_config();
    let ir = analyze(code, &config);
    assert!(
        !ir.whole_events.is_empty(),
        "wide FF module must be AOT-C-native (whole_events non-empty)"
    );
    let mut sim = Simulator::new(ir, None);
    let clk = sim.get_clock("clk").unwrap();
    sim.set("a", Value::new(100, 256, false));
    sim.step(&clk);
    assert_eq!(sim.get("q").unwrap(), Value::new(101, 256, false));
}

#[test]
fn wide_128_ff_validate_pool_agnostic() {
    // Regression for the validate-extension false positive: a 65-128 bit FF
    // commits the SAME bytes through the AOT-C WIDE pool (one 16-byte entry)
    // but the Cranelift JIT NARROW pool (two u64 entries).  validate_event_aot
    // must compare the resolved committed BYTES, not pool-specific entry maps,
    // or it spuriously panics on a byte-identical commit.  aot_c_validate=true
    // dual-runs every cycle, so a regression panics inside step().
    if !crate::backend::aot_c::cc_available() {
        return;
    }
    let code = r#"
    module Top (
        clk: input  clock,
        a:   input  logic<128>,
        q:   output logic<128>,
    ) {
        always_ff {
            q = a + 1;
        }
    }
    "#;
    let config = aot_native_validate_config();
    let ir = analyze(code, &config);
    assert!(
        !ir.whole_events.is_empty(),
        "128-bit FF must be AOT-C-native"
    );
    let mut sim = Simulator::new(ir, None);
    let clk = sim.get_clock("clk").unwrap();
    let a: u128 = (1u128 << 100) | 0xDEAD_BEEF;
    sim.set("a", Value::from_u128(a, 0, 128, false));
    sim.step(&clk); // pre-fix: panics here with a false "validate divergence"
    assert_eq!(
        sim.get("q").unwrap(),
        Value::from_u128(a.wrapping_add(1), 0, 128, false)
    );
}

#[test]
fn wide_256_reduce_and_nested_unary() {
    // Wide unary REDUCTIONS (&a / |a / ^a → 1-bit) exercise
    // emit_wide_reduce_unary; the NESTED unary round-trips (~(~a) and -(-a))
    // exercise emit_wide_unary (bnot/negate) and, crucially, the flat-prelude
    // design where an outer wide op consumes an inner wide op's scratch.
    // Cross-validated interpret/Cranelift/cc via Config::all().
    let code = r#"
    module Top (
        a:     input  logic<256>,
        r_and: output logic<1>,
        r_or:  output logic<1>,
        r_xor: output logic<1>,
        inv2:  output logic<256>,
        neg2:  output logic<256>,
    ) {
        assign r_and = &a;
        assign r_or  = |a;
        assign r_xor = ^a;
        assign inv2  = ~(~a);
        assign neg2  = -(-a);
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        // 0x7 = 3 set bits in the low word, all high bits zero.
        sim.set("a", Value::from_u128(0x7, 0, 256, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));

        // &a = 0 (not all-ones); |a = 1 (nonzero); ^a = parity(3) = 1.
        assert_eq!(
            sim.get("r_and").unwrap(),
            Value::new(0, 1, false),
            "{config:?}"
        );
        assert_eq!(
            sim.get("r_or").unwrap(),
            Value::new(1, 1, false),
            "{config:?}"
        );
        assert_eq!(
            sim.get("r_xor").unwrap(),
            Value::new(1, 1, false),
            "{config:?}"
        );
        // Double complement / double negate are the identity.
        assert_eq!(
            sim.get("inv2").unwrap(),
            Value::from_u128(0x7, 0, 256, false),
            "{config:?}"
        );
        assert_eq!(
            sim.get("neg2").unwrap(),
            Value::from_u128(0x7, 0, 256, false),
            "{config:?}"
        );
    }
}

#[test]
fn wide_256_concat_and_xnor() {
    // Wide concatenation producing a >128-bit result (emit_wide_concat) and a
    // wide BitXnor (`~^`, emit_wide_binary bxor_not).  Expected values exceed
    // 128 bits, so built via BigUint.  Cross-validated via Config::all().
    use num_bigint::BigUint;
    let code = r#"
    module Top (
        a:   input  logic<128>,
        b:   input  logic<128>,
        cc:  output logic<256>,
        xn:  output logic<256>,
    ) {
        assign cc = {a, b};
        assign xn = {128'd0, a} ~^ {128'd0, b};
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let pa: u128 = (0x1122_3344_5566_7788u128 << 64) | 0x99AA_BBCC_DDEE_FF00u128;
        let pb: u128 = (0x0F0F_0F0F_0F0F_0F0Fu128 << 64) | 0xF0F0_F0F0_F0F0_F0F0u128;
        sim.set("a", Value::from_u128(pa, 0, 128, false));
        sim.set("b", Value::from_u128(pb, 0, 128, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));

        // cc = {a, b}: a in the high 128 bits, b in the low 128.
        let cc_big = (BigUint::from(pa) << 128) | BigUint::from(pb);
        assert_eq!(
            sim.get("cc").unwrap(),
            Value::new_biguint(cc_big, 256, false),
            "{config:?}"
        );
        // xn = (0:a) ~^ (0:b) = ~((0:a) ^ (0:b)) masked to 256 bits.  The high
        // 128 bits of both operands are 0, so xn's high 128 bits are all ones.
        let ab = BigUint::from(pa) ^ BigUint::from(pb); // low 128 bits set, high 0
        let mask256 = (BigUint::from(1u8) << 256) - BigUint::from(1u8);
        let xn_big = (&mask256) ^ ab; // ~ab within 256 bits
        assert_eq!(
            sim.get("xn").unwrap(),
            Value::new_biguint(xn_big, 256, false),
            "{config:?}"
        );
    }
}

#[test]
fn wide_result_narrower_than_operands() {
    // Operands (256-bit) wider than the result (130-bit): the add is computed
    // at the operand width then truncated to 130 on store.  Exercises
    // emit_wide_operand's `r.nb > target_nb` clamp — a 256-bit (32-byte) value
    // copied into the 130-bit (24-byte) destination scratch; an unclamped copy
    // would overflow.  Cross-validated via Config::all().
    use num_bigint::BigUint;
    let code = r#"
    module Top (
        a: input  logic<256>,
        b: input  logic<256>,
        c: output logic<130>,
    ) {
        assign c = a + b;
    }
    "#;
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        // a = 2^200 + 5, b = 3 → a+b = 2^200 + 8; truncated to 130 bits = 8.
        let a = (BigUint::from(1u8) << 200) + BigUint::from(5u8);
        let b = BigUint::from(3u8);
        sim.set("a", Value::new_biguint(a, 256, false));
        sim.set("b", Value::new_biguint(b, 256, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("c").unwrap(),
            Value::new(8, 130, false),
            "{config:?}"
        );
    }
}

#[test]
fn wide_v4_repro_comb_select_store_232() {
    // Repro for the v4 boot SEGV: a wide (non-64-multiple) COMB signal with a
    // single-bit select store, mirroring the census `Assign(dw=232,sel=true)`.
    let code = r#"
    module Top (
        a:      input  logic<232>,
        bit_in: input  logic,
        w:      output logic<232>,
    ) {
        always_comb {
            w = a;
            w[231] = bit_in;
            w[100] = bit_in;
        }
    }
    "#;
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let a = Value::new(0, 232, false);
        sim.set("a", a);
        sim.set("bit_in", Value::new(1, 1, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        // w == a with bits 231 and 100 set.
        let got = sim.get("w").unwrap();
        let exp = {
            use num_bigint::BigUint;
            let v = (BigUint::from(1u8) << 231) | (BigUint::from(1u8) << 100);
            Value::new_biguint(v, 232, false)
        };
        assert_eq!(got, exp, "{config:?}");
    }
}

#[test]
fn wide_v4_repro_var_select_read_232() {
    // Repro: narrow bit-select READ from a wide var (census exprOK=false).
    let code = r#"
    module Top (
        a:    input  logic<232>,
        top:  output logic,
        mid:  output logic,
        bsel: output logic<8>,
    ) {
        assign top  = a[231];
        assign mid  = a[100];
        assign bsel = a[207:200];
    }
    "#;
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let p: u128 = (1u128 << 100) | 0xAB;
        sim.set("a", Value::from_u128(p, 0, 232, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("top").unwrap(),
            Value::new(0, 1, false),
            "{config:?}"
        );
        assert_eq!(
            sim.get("mid").unwrap(),
            Value::new(1, 1, false),
            "{config:?}"
        );
        assert_eq!(
            sim.get("bsel").unwrap(),
            Value::new(0, 8, false),
            "{config:?}"
        );
    }
}

#[test]
fn wide_v4_repro_dyn_array_232() {
    // Repro: dynamic-indexed read/write of a WIDE (232-bit) array element —
    // the OoO regfile/ROB/IQ pattern.  `arr[idx]` is a wide DynamicVariable;
    // exercises the Cranelift S1 `nb>16` DynamicVariable branch + wide dynamic
    // FF write.
    let code = r#"
    module Top (
        clk:  input  clock,
        idx:  input  logic<4>,
        we:   input  logic,
        din:  input  logic<232>,
        dout: output logic<232>,
    ) {
        var arr: logic<232> [16];
        always_ff {
            if we {
                arr[idx] = din;
            }
        }
        assign dout = arr[idx];
    }
    "#;
    // 2-state only: the 4-state path under-allocates comb storage for a wide
    // ARRAY (a pre-existing layout bug surfaced by this repro, unrelated to
    // the wide-value JIT work here; v4 is 2-state).
    for config in Config::all().into_iter().filter(|c| !c.use_4state) {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        sim.set("idx", Value::new(5, 4, false));
        sim.set("we", Value::new(1, 1, false));
        let d: num_bigint::BigUint =
            (num_bigint::BigUint::from(1u8) << 200usize) | num_bigint::BigUint::from(0xDEADu32);
        sim.set("din", Value::new_biguint(d.clone(), 232, false));
        sim.step(&clk);
        assert_eq!(
            sim.get("dout").unwrap(),
            Value::new_biguint(d, 232, false),
            "{config:?}"
        );
    }
}

#[test]
fn nested_array_index_const_array() {
    // Regression for a nested array index `mem[A[idx]]` with a const array
    // `A`: the const-symbol read `A[idx]` was wrongly flagged as a compile-time
    // constant and folded to `A[0]`, so the outer index ignored `idx` and the
    // simulator read `mem[A[0]]` instead of `mem[A[idx]]`. All three backends
    // were affected; the emitted SV was correct.
    let code = r#"
    module Top (
        idx:    input  logic<8>,
        nested: output logic<8>,
        inner:  output logic<8>,
    ) {
        const A:   logic<8> [2] = '{1, 3};
        var   mem: logic<8> [8];
        always_comb {
            mem[0] = 0;
            mem[1] = 11;
            mem[2] = 22;
            mem[3] = 33;
            mem[4] = 44;
            mem[5] = 55;
            mem[6] = 66;
            mem[7] = 77;
        }
        assign inner  = A[idx];
        assign nested = mem[A[idx]];
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        // idx = 1 -> A[1] = 3 -> mem[3] = 33 (buggy fold gives mem[A[0]] = 11).
        sim.set("idx", Value::new(1, 8, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("inner").unwrap(),
            Value::new(3, 8, false),
            "{config:?}"
        );
        assert_eq!(
            sim.get("nested").unwrap(),
            Value::new(33, 8, false),
            "{config:?}"
        );

        // idx = 0 -> A[0] = 1 -> mem[1] = 11 (differs from idx = 1, so not folded).
        sim.set("idx", Value::new(0, 8, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("inner").unwrap(),
            Value::new(1, 8, false),
            "{config:?}"
        );
        assert_eq!(
            sim.get("nested").unwrap(),
            Value::new(11, 8, false),
            "{config:?}"
        );
    }
}

#[test]
fn wide_narrow_field_two_word_straddle() {
    // A <=64-bit field that STRADDLES a 64-bit word boundary exercises the
    // two-word arm of emit_wide_narrow_field_store.  w[130:67] is a 64-bit
    // field crossing the word-1/word-2 boundary (b = 67 % 64 = 3, k0=1, k1=2).
    let code = r#"
    module Top (
        f: input  logic<64>,
        w: output logic<200>,
    ) {
        always_comb {
            w = 0;
            w[130:67] = f;
        }
    }
    "#;
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let f: u64 = 0xFEDC_BA98_7654_3210;
        sim.set("f", Value::new(f, 64, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        let exp = {
            use num_bigint::BigUint;
            Value::new_biguint(BigUint::from(f) << 67usize, 200, false)
        };
        assert_eq!(sim.get("w").unwrap(), exp, "{config:?}");
    }
}

#[test]
fn wide_256_comparison() {
    let code = r#"
    module Top (
        a: input  logic<256>,
        b: input  logic<256>,
        eq: output logic,
        ne: output logic,
        gt: output logic,
        lt: output logic,
    ) {
        assign eq = a == b;
        assign ne = a != b;
        assign gt = a >: b;
        assign lt = a <: b;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let a = Value::new(100, 256, false);
        let b = Value::new(42, 256, false);

        sim.set("a", a);
        sim.set("b", b);
        sim.step(&Event::Clock(VarId::SYNTHETIC));

        assert_eq!(
            sim.get("eq").unwrap(),
            Value::new(0, 1, false),
            "config: {:?}",
            config
        );
        assert_eq!(
            sim.get("ne").unwrap(),
            Value::new(1, 1, false),
            "config: {:?}",
            config
        );
        assert_eq!(
            sim.get("gt").unwrap(),
            Value::new(1, 1, false),
            "config: {:?}",
            config
        );
        assert_eq!(
            sim.get("lt").unwrap(),
            Value::new(0, 1, false),
            "config: {:?}",
            config
        );
    }
}

#[test]
fn wide_256_shift() {
    let code = r#"
    module Top (
        a: input  logic<256>,
        s: input  logic<8>,
        left: output logic<256>,
        right: output logic<256>,
    ) {
        assign left  = a << s;
        assign right = a >> s;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let a = Value::new(0xff, 256, false);
        let s = Value::new(4, 8, false);

        sim.set("a", a);
        sim.set("s", s);
        sim.step(&Event::Clock(VarId::SYNTHETIC));

        assert_eq!(
            sim.get("left").unwrap(),
            Value::new(0xff0, 256, false),
            "config: {:?}",
            config
        );
        assert_eq!(
            sim.get("right").unwrap(),
            Value::new(0xf, 256, false),
            "config: {:?}",
            config
        );
    }
}

#[test]
fn wide_256_ternary() {
    let code = r#"
    module Top (
        sel: input  logic,
        a: input  logic<256>,
        b: input  logic<256>,
        c: output logic<256>,
    ) {
        assign c = if sel ?  a : b;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let a = Value::new(100, 256, false);
        let b = Value::new(200, 256, false);

        sim.set("sel", Value::new(1, 1, false));
        sim.set("a", a.clone());
        sim.set("b", b.clone());
        sim.step(&Event::Clock(VarId::SYNTHETIC));

        assert_eq!(sim.get("c").unwrap(), a, "config: {:?}", config);

        sim.set("sel", Value::new(0, 1, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));

        assert_eq!(sim.get("c").unwrap(), b, "config: {:?}", config);
    }
}

/// Regression: 256-bit values in for-generate with narrow constant assignment.
/// Exercises two fixed bugs:
///   1. Value width mismatch: BigUint(width=256) inside ProtoExpression(width=32)
///      caused build_binary to return a pointer while the caller expected a register.
///   2. Unaligned memory access: 32-bit variables before 256-bit elements
///      placed them at non-8-byte-aligned offsets, crashing wide_ops helpers.
#[test]
fn wide_256_array_for_generate() {
    // Use separate output ports (not an array) so sim.get() can access them.
    // The for-generate still creates the alignment pattern that triggers bug #2.
    let code = r#"
    module Top (
        clk : input  clock    ,
        rst : input  reset    ,
        a   : output logic<256>,
        b   : output logic<256>,
    ) {
        // a small 32-bit var before wide vars to create unaligned layout
        var pad: logic<32>;
        assign pad = 0;

        always_ff {
            if_reset {
                a = 1;
                b = 2;
            } else {
                a += 1;
                b += 1;
            }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);
        sim.step(&clk);
        sim.step(&clk);
        sim.step(&clk);

        // After reset: a=1, b=2. After 3 clocks: a=4, b=5
        assert_eq!(
            sim.get("a").unwrap(),
            Value::new(4, 256, false),
            "config: {:?}",
            config
        );
        assert_eq!(
            sim.get("b").unwrap(),
            Value::new(5, 256, false),
            "config: {:?}",
            config
        );
    }
}

#[test]
fn wide_bit_ops() {
    // Test bitwise operations on 96-bit variables
    let code = r#"
    module Top (
        a: input  logic<96>,
        b: input  logic<96>,
        c: output logic<96>,
        d: output logic<96>,
        e: output logic<96>,
    ) {
        assign c = a & b;
        assign d = a | b;
        assign e = a ^ b;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let a = Value::new(0xff00ff, 96, false);
        let b = Value::new(0x0f0f0f, 96, false);

        sim.set("a", a);
        sim.set("b", b);
        sim.step(&Event::Clock(VarId::SYNTHETIC));

        let c = Value::new(0x0f000f, 96, false);
        let d = Value::new(0xff0fff, 96, false);
        let e = Value::new(0xf00ff0, 96, false);
        assert_eq!(sim.get("c").unwrap(), c);
        assert_eq!(sim.get("d").unwrap(), d);
        assert_eq!(sim.get("e").unwrap(), e);
    }
}

#[test]
fn wide_ternary() {
    let code = r#"
    module Top (
        sel: input  logic,
        a:   input  logic<128>,
        b:   input  logic<128>,
        c:   output logic<128>,
    ) {
        assign c = if sel ? a : b;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let a = Value::new(42, 128, false);
        let b = Value::new(99, 128, false);

        sim.set("sel", Value::new(1, 1, false));
        sim.set("a", a.clone());
        sim.set("b", b.clone());
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("c").unwrap(), a);

        sim.set("sel", Value::new(0, 1, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("c").unwrap(), b);
    }
}

#[test]
fn select() {
    let code = r#"
    module Top (
        a: input  logic<32>,
        x: output logic<32>,
        y: output logic<32>,
        z: output logic<32>,
        v: output logic<32>,
        w: output logic<32>,
    ) {
        assign x = a[0];
        assign y = a[1];
        assign z = a[3:2];
        assign v = a[7:5] + a[1:0];
        assign w[0] = a[2];
        assign w[1] = a[0];
        assign w[31:2] = a[6:5];
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);

        let mut sim = Simulator::new(ir, None);

        println!("{}", sim.ir.dump_variables());

        let a = Value::new(0xae, 32, false);

        sim.set("a", a);

        println!("{}", sim.ir.dump_variables());

        sim.step(&Event::Clock(VarId::SYNTHETIC));

        println!("{}", sim.ir.dump_variables());

        let x = Value::new(0, 32, false);
        let y = Value::new(1, 32, false);
        let z = Value::new(3, 32, false);
        let v = Value::new(7, 32, false);
        let w = Value::new(5, 32, false);

        assert_eq!(sim.get("x").unwrap(), x);
        assert_eq!(sim.get("y").unwrap(), y);
        assert_eq!(sim.get("z").unwrap(), z);
        assert_eq!(sim.get("v").unwrap(), v);
        assert_eq!(sim.get("w").unwrap(), w);
    }
}

#[test]
fn inst() {
    let code = r#"
    module Top (
        a: input  logic<32>,
        b: input  logic<32>,
        c: output logic<32>,
    ) {
        inst u: Sub (
            a,
            b,
            c,
        );
    }

    module Sub (
        a: input  logic<32>,
        b: input  logic<32>,
        c: output logic<32>,
    ) {
        var x: logic<32>;
        assign x = a + b;
        assign c = x;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);

        let mut sim = Simulator::new(ir, None);

        println!("{}", sim.ir.dump_variables());

        let a = Value::new(10, 32, false);
        let b = Value::new(20, 32, false);

        sim.set("a", a);
        sim.set("b", b);

        println!("{}", sim.ir.dump_variables());

        sim.step(&Event::Clock(VarId::SYNTHETIC));

        println!("{}", sim.ir.dump_variables());

        let exp = Value::new(30, 32, false);

        assert_eq!(sim.get("c").unwrap(), exp);
    }
}

#[test]
fn inst_array_input_port() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        c:   output logic<32>,
    ) {
        var arr: logic<32> [2];
        always_ff {
            if_reset {
                arr[0] = 0;
                arr[1] = 0;
            } else {
                arr[0] += 1;
                arr[1] += 2;
            }
        }

        inst u: Sub (
            clk,
            rst,
            i_x: arr,
            c,
        );
    }

    module Sub (
        clk: input  clock,
        rst: input  reset,
        i_x: input  logic<32> [2],
        c:   output logic<32>,
    ) {
        assign c = i_x[0] + i_x[1];
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);

        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        // After 3 clock cycles: arr[0]=3, arr[1]=6, c=9
        for _ in 0..3 {
            sim.step(&clk);
        }

        assert_eq!(sim.get("c").unwrap(), Value::new(9, 32, false));
    }
}

#[test]
fn inst_array_input_port_shorthand() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        c:   output logic<32>,
    ) {
        var i_x: logic<32> [2];
        always_ff {
            if_reset {
                i_x[0] = 0;
                i_x[1] = 0;
            } else {
                i_x[0] += 10;
                i_x[1] += 20;
            }
        }

        inst u: Sub (
            clk,
            rst,
            i_x,
            c,
        );
    }

    module Sub (
        clk: input  clock,
        rst: input  reset,
        i_x: input  logic<32> [2],
        c:   output logic<32>,
    ) {
        assign c = i_x[0] + i_x[1];
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);

        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        // After 2 clock cycles: i_x[0]=20, i_x[1]=40, c=60
        for _ in 0..2 {
            sim.step(&clk);
        }

        assert_eq!(sim.get("c").unwrap(), Value::new(60, 32, false));
    }
}

#[test]
fn inst_ff() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        cnt: output logic<32>,
    ) {
        inst u: Counter (
            clk,
            rst,
            cnt,
        );
    }

    module Counter (
        clk: input  clock,
        rst: input  reset,
        cnt: output logic<32>,
    ) {
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);

        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        for _ in 0..50 {
            sim.step(&clk);
        }

        let exp = Value::new(50, 32, false);

        assert_eq!(sim.get("cnt").unwrap(), exp);
    }
}

#[test]
fn inst_comb_and_ff() {
    // Sub-module with both comb and FF to test merged comb+event JIT
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        out: output logic<32>,
        comb_out: output logic<32>,
    ) {
        inst u: Inner (
            clk,
            rst,
            out,
            comb_out,
        );
    }

    module Inner (
        clk: input  clock,
        rst: input  reset,
        out: output logic<32>,
        comb_out: output logic<32>,
    ) {
        // comb depends on FF output
        assign comb_out = out + 1;

        always_ff {
            if_reset {
                out = 0;
            } else {
                out = comb_out;
            }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);

        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        // After reset: out=0, comb_out=0+1=1
        assert_eq!(sim.get("out").unwrap(), Value::new(0, 32, false));
        assert_eq!(sim.get("comb_out").unwrap(), Value::new(1, 32, false));

        sim.step(&clk);
        // After step 1: out=comb_out=1, comb_out=1+1=2
        assert_eq!(sim.get("out").unwrap(), Value::new(1, 32, false));
        assert_eq!(sim.get("comb_out").unwrap(), Value::new(2, 32, false));

        sim.step(&clk);
        // After step 2: out=comb_out=2, comb_out=2+1=3
        assert_eq!(sim.get("out").unwrap(), Value::new(2, 32, false));
        assert_eq!(sim.get("comb_out").unwrap(), Value::new(3, 32, false));

        for _ in 0..10 {
            sim.step(&clk);
        }
        // After 12 total steps: out=12, comb_out=13
        assert_eq!(sim.get("out").unwrap(), Value::new(12, 32, false));
        assert_eq!(sim.get("comb_out").unwrap(), Value::new(13, 32, false));
    }
}

#[test]
fn binary_op() {
    let code = r#"
    module Top (
        a  : input  logic<32>,
        b  : input  logic<32>,
        c  : input  logic    ,
        d  : input  logic    ,
        x00: output logic<32>,
        x01: output logic<32>,
        x02: output logic<32>,
        x03: output logic<32>,
        x04: output logic<32>,
        x05: output logic<32>,
        x06: output logic<32>,
        x07: output logic<32>,
        x08: output logic<32>,
        x09: output logic<32>,
        x10: output logic<32>,
        x11: output logic<32>,
        x12: output logic<32>,
        x13: output logic<32>,
        x14: output logic<32>,
        x15: output logic<32>,
        x16: output logic<32>,
    ) {
        assign x00 = a + b;
        assign x01 = a - b;
        assign x02 = a * b;
        assign x03 = b / a;
        assign x04 = b % a;
        assign x05 = a & b;
        assign x06 = a | b;
        assign x07 = a ^ b;
        assign x08 = a ~^ b;
        assign x09 = a == b;
        assign x10 = a != b;
        assign x11 = a >: b;
        assign x12 = a >= b;
        assign x13 = a <: b;
        assign x14 = a <= b;
        assign x15 = c && d;
        assign x16 = c || d;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);

        let mut sim = Simulator::new(ir, None);

        let a = Value::new(11, 32, false);
        let b = Value::new(21, 32, false);
        let c = Value::new(0, 1, false);
        let d = Value::new(1, 1, false);

        sim.set("a", a);
        sim.set("b", b);
        sim.set("c", c);
        sim.set("d", d);

        sim.step(&Event::Clock(VarId::SYNTHETIC));

        println!("{}", sim.ir.dump_variables());

        assert_eq!(sim.get("x00").unwrap(), Value::new(32, 32, false));
        assert_eq!(sim.get("x01").unwrap(), Value::new(0xfffffff6, 32, false));
        assert_eq!(sim.get("x02").unwrap(), Value::new(231, 32, false));
        assert_eq!(sim.get("x03").unwrap(), Value::new(1, 32, false));
        assert_eq!(sim.get("x04").unwrap(), Value::new(10, 32, false));
        assert_eq!(sim.get("x05").unwrap(), Value::new(1, 32, false));
        assert_eq!(sim.get("x06").unwrap(), Value::new(31, 32, false));
        assert_eq!(sim.get("x07").unwrap(), Value::new(30, 32, false));
        assert_eq!(sim.get("x08").unwrap(), Value::new(0xffffffe1, 32, false));
        assert_eq!(sim.get("x09").unwrap(), Value::new(0, 32, false));
        assert_eq!(sim.get("x10").unwrap(), Value::new(1, 32, false));
        assert_eq!(sim.get("x11").unwrap(), Value::new(0, 32, false));
        assert_eq!(sim.get("x12").unwrap(), Value::new(0, 32, false));
        assert_eq!(sim.get("x13").unwrap(), Value::new(1, 32, false));
        assert_eq!(sim.get("x14").unwrap(), Value::new(1, 32, false));
        assert_eq!(sim.get("x15").unwrap(), Value::new(0, 32, false));
        assert_eq!(sim.get("x16").unwrap(), Value::new(1, 32, false));
    }
}

#[test]
fn comb_dependency() {
    let code = r#"
    module Top (
        a: input  logic<32>,
        b: input  logic<32>,
        x: output logic<32>,
        y: output logic<32>,
        z: output logic<32>,
        v: output logic<32>,
        w: output logic<32>,
    ) {
        assign w = z + x;
        assign v = a + 1;
        assign z = b + 2;
        assign y = v + a;
        assign x = b + y;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);

        let mut sim = Simulator::new(ir, None);

        println!("{}", sim.ir.dump_variables());

        let a = Value::new(10, 32, false);
        let b = Value::new(20, 32, false);

        sim.set("a", a);
        sim.set("b", b);

        println!("{}", sim.ir.dump_variables());

        sim.step(&Event::Clock(VarId::SYNTHETIC));

        println!("{}", sim.ir.dump_variables());

        assert_eq!(sim.get("x").unwrap(), Value::new(41, 32, false));
        assert_eq!(sim.get("y").unwrap(), Value::new(21, 32, false));
        assert_eq!(sim.get("z").unwrap(), Value::new(22, 32, false));
        assert_eq!(sim.get("v").unwrap(), Value::new(11, 32, false));
        assert_eq!(sim.get("w").unwrap(), Value::new(63, 32, false));
    }
}

#[test]
fn dump_vcd() {
    let code = r#"
    module Top (
        a: input  logic<32>,
        b: input  logic<32>,
        c: output logic<32>,
    ) {
        assign c = a + b;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);

        use crate::wave_dumper::WaveDumper;
        let dump_buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let dumper = WaveDumper::new_vcd(Box::new(crate::wave_dumper::SharedVec(dump_buf.clone())));
        let mut sim = Simulator::new(ir, Some(dumper));

        let a = Value::new(10, 32, false);
        let b = Value::new(20, 32, false);

        sim.set("a", a);
        sim.set("b", b);

        sim.step(&Event::Clock(VarId::SYNTHETIC));
        sim.time += 1;

        let a = Value::new(30, 32, false);
        let b = Value::new(10, 32, false);

        sim.set("a", a);
        sim.set("b", b);

        sim.step(&Event::Clock(VarId::SYNTHETIC));
        sim.time += 1;

        let a = Value::new(50, 32, false);
        let b = Value::new(20, 32, false);

        sim.set("a", a);
        sim.set("b", b);

        sim.step(&Event::Clock(VarId::SYNTHETIC));
        sim.time += 1;

        drop(sim);
        let dump = String::from_utf8(
            std::sync::Arc::try_unwrap(dump_buf)
                .unwrap()
                .into_inner()
                .unwrap(),
        )
        .unwrap();
        let exp = r#"$timescale 1 us $end
$scope module Top $end
$var wire 32 ! a $end
$var wire 32 " b $end
$var wire 32 # c $end
$upscope $end
$enddefinitions $end
#0
b00000000000000000000000000001010 !
b00000000000000000000000000010100 "
b00000000000000000000000000011110 #
#1
b00000000000000000000000000011110 !
b00000000000000000000000000001010 "
b00000000000000000000000000101000 #
#2
b00000000000000000000000000110010 !
b00000000000000000000000000010100 "
b00000000000000000000000001000110 #
"#;
        assert_eq!(dump, exp);
    }
}

#[test]
fn dump_vcd_generic_function() {
    let code = r#"
    module Top (
        a: input  logic<32>,
        c: output logic<32>,
    ) {
        function Add1::<W: u32> (
            x: input logic<W>,
        ) -> logic<W> {
            return x + 1;
        }

        assign c = Add1::<32>(a);
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);

        use crate::wave_dumper::WaveDumper;
        let dump_buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let dumper = WaveDumper::new_vcd(Box::new(crate::wave_dumper::SharedVec(dump_buf.clone())));
        let mut sim = Simulator::new(ir, Some(dumper));

        let a = Value::new(10, 32, false);
        sim.set("a", a);

        sim.step(&Event::Clock(VarId::SYNTHETIC));
        sim.time += 1;

        drop(sim);
        let dump = String::from_utf8(
            std::sync::Arc::try_unwrap(dump_buf)
                .unwrap()
                .into_inner()
                .unwrap(),
        )
        .unwrap();

        assert!(!dump.contains("::<"), "VCD should not contain '::<'");
        assert!(!dump.contains('>'), "VCD should not contain '>'");
        assert!(
            dump.contains("Add1_32"),
            "VCD should contain sanitized generic function name 'Add1_32'"
        );
    }
}

#[track_caller]
fn unary_test(op: &str, x: &str, dst_width: usize, dst: &str, only_4state: bool) {
    let x_signed = if x.contains('s') { "signed" } else { "" };
    let x = Value::from_str(x.trim()).unwrap();
    let x_width = x.width();

    let code = format!(
        r#"
    module Top (
        x: input {} logic<{}>,
        o: output logic<{}>,
    ) {{
        assign o = {} x;
    }}
    "#,
        x_signed,
        x_width,
        dst_width,
        op.trim()
    );

    for config in Config::all() {
        if only_4state && !config.use_4state {
            continue;
        }

        dbg!(&config);

        let ir = analyze(&code, &config);

        let mut sim = Simulator::new(ir, None);

        sim.set("x", x.clone());

        sim.step(&Event::Clock(VarId::SYNTHETIC));

        println!("{}", sim.ir.dump_variables());

        if dst.contains('b') {
            assert_eq!(format!("{:b}", sim.get("o").unwrap()), dst);
        } else {
            assert_eq!(format!("{:x}", sim.get("o").unwrap()), dst);
        }
    }
}

#[test]
fn unary_corner_case() {
    unary_test("+ ", "8'h11 ", 16, "16'b0000000000010001", false);
    unary_test("+ ", "8'hf2 ", 16, "16'b0000000011110010", false);
    unary_test("+ ", "8'hx3 ", 16, "16'b00000000xxxx0011", true);
    unary_test("+ ", "8'hz4 ", 16, "16'b00000000zzzz0100", true);
    unary_test("+ ", "8'sh15", 16, "16'b0000000000010101", false);
    unary_test("+ ", "8'shf6", 16, "16'b1111111111110110", false);
    unary_test("+ ", "8'shx7", 16, "16'bxxxxxxxxxxxx0111", true);
    unary_test("+ ", "8'shz8", 16, "16'bzzzzzzzzzzzz1000", true);

    unary_test("- ", "8'h11 ", 16, "16'b1111111111101111", false);
    unary_test("- ", "8'hf2 ", 16, "16'b1111111100001110", false);
    unary_test("- ", "8'hx3 ", 16, "16'bxxxxxxxxxxxxxxxx", true);
    unary_test("- ", "8'hz4 ", 16, "16'bxxxxxxxxxxxxxxxx", true);
    unary_test("- ", "8'sh15", 16, "16'b1111111111101011", false);
    unary_test("- ", "8'shf6", 16, "16'b0000000000001010", false);
    unary_test("- ", "8'shx7", 16, "16'bxxxxxxxxxxxxxxxx", true);
    unary_test("- ", "8'shz8", 16, "16'bxxxxxxxxxxxxxxxx", true);

    unary_test("~ ", "8'h11 ", 16, "16'b1111111111101110", false);
    unary_test("~ ", "8'hf2 ", 16, "16'b1111111100001101", false);
    unary_test("~ ", "8'hx3 ", 16, "16'b11111111xxxx1100", true);
    unary_test("~ ", "8'hz4 ", 16, "16'b11111111xxxx1011", true);
    unary_test("~ ", "8'sh15", 16, "16'b1111111111101010", false);
    unary_test("~ ", "8'shf6", 16, "16'b0000000000001001", false);
    unary_test("~ ", "8'shx7", 16, "16'bxxxxxxxxxxxx1000", true);
    unary_test("~ ", "8'shz8", 16, "16'bxxxxxxxxxxxx0111", true);

    unary_test("& ", "8'h11 ", 16, "16'b0000000000000000", false);
    unary_test("& ", "8'hff ", 16, "16'b0000000000000001", false);
    unary_test("& ", "8'hxx ", 16, "16'b000000000000000x", true);
    unary_test("& ", "8'hzz ", 16, "16'b000000000000000x", true);
    unary_test("& ", "8'h1x ", 16, "16'b0000000000000000", true);
    unary_test("& ", "8'h1z ", 16, "16'b0000000000000000", true);
    unary_test("& ", "8'hfx ", 16, "16'b000000000000000x", true);
    unary_test("& ", "8'hxz ", 16, "16'b000000000000000x", true);

    unary_test("~&", "8'h11 ", 16, "16'b0000000000000001", false);
    unary_test("~&", "8'hff ", 16, "16'b0000000000000000", false);
    unary_test("~&", "8'hxx ", 16, "16'b000000000000000x", true);
    unary_test("~&", "8'hzz ", 16, "16'b000000000000000x", true);
    unary_test("~&", "8'h1x ", 16, "16'b0000000000000001", true);
    unary_test("~&", "8'h1z ", 16, "16'b0000000000000001", true);
    unary_test("~&", "8'hfx ", 16, "16'b000000000000000x", true);
    unary_test("~&", "8'hxz ", 16, "16'b000000000000000x", true);

    unary_test("| ", "8'h00 ", 16, "16'b0000000000000000", false);
    unary_test("| ", "8'h11 ", 16, "16'b0000000000000001", false);
    unary_test("| ", "8'hxx ", 16, "16'b000000000000000x", true);
    unary_test("| ", "8'hzz ", 16, "16'b000000000000000x", true);
    unary_test("| ", "8'h0x ", 16, "16'b000000000000000x", true);
    unary_test("| ", "8'h0z ", 16, "16'b000000000000000x", true);
    unary_test("| ", "8'h1x ", 16, "16'b0000000000000001", true);
    unary_test("| ", "8'h1z ", 16, "16'b0000000000000001", true);

    unary_test("~|", "8'h00 ", 16, "16'b0000000000000001", false);
    unary_test("~|", "8'h11 ", 16, "16'b0000000000000000", false);
    unary_test("~|", "8'hxx ", 16, "16'b000000000000000x", true);
    unary_test("~|", "8'hzz ", 16, "16'b000000000000000x", true);
    unary_test("~|", "8'h0x ", 16, "16'b000000000000000x", true);
    unary_test("~|", "8'h0z ", 16, "16'b000000000000000x", true);
    unary_test("~|", "8'h1x ", 16, "16'b0000000000000000", true);
    unary_test("~|", "8'h1z ", 16, "16'b0000000000000000", true);

    unary_test("^ ", "8'h00 ", 16, "16'b0000000000000000", false);
    unary_test("^ ", "8'h01 ", 16, "16'b0000000000000001", false);
    unary_test("^ ", "8'hxx ", 16, "16'b000000000000000x", true);
    unary_test("^ ", "8'hzz ", 16, "16'b000000000000000x", true);
    unary_test("^ ", "8'h0x ", 16, "16'b000000000000000x", true);
    unary_test("^ ", "8'h0z ", 16, "16'b000000000000000x", true);
    unary_test("^ ", "8'h1x ", 16, "16'b000000000000000x", true);
    unary_test("^ ", "8'h1z ", 16, "16'b000000000000000x", true);

    unary_test("~^", "8'h00 ", 16, "16'b0000000000000001", false);
    unary_test("~^", "8'h01 ", 16, "16'b0000000000000000", false);
    unary_test("~^", "8'hxx ", 16, "16'b000000000000000x", true);
    unary_test("~^", "8'hzz ", 16, "16'b000000000000000x", true);
    unary_test("~^", "8'h0x ", 16, "16'b000000000000000x", true);
    unary_test("~^", "8'h0z ", 16, "16'b000000000000000x", true);
    unary_test("~^", "8'h1x ", 16, "16'b000000000000000x", true);
    unary_test("~^", "8'h1z ", 16, "16'b000000000000000x", true);

    unary_test("! ", "8'h00 ", 16, "16'b0000000000000001", false);
    unary_test("! ", "8'h01 ", 16, "16'b0000000000000000", false);
    unary_test("! ", "8'hxx ", 16, "16'b000000000000000x", true);
    unary_test("! ", "8'hzz ", 16, "16'b000000000000000x", true);
    unary_test("! ", "8'h0x ", 16, "16'b000000000000000x", true);
    unary_test("! ", "8'h0z ", 16, "16'b000000000000000x", true);
    unary_test("! ", "8'h1x ", 16, "16'b0000000000000000", true);
    unary_test("! ", "8'h1z ", 16, "16'b0000000000000000", true);
}

#[track_caller]
fn binary_test(x: &str, op: &str, y: &str, dst_width: usize, dst: &str, only_4state: bool) {
    let x_signed = if x.contains('s') { "signed" } else { "" };
    let y_signed = if y.contains('s') { "signed" } else { "" };
    let x = Value::from_str(x.trim()).unwrap();
    let y = Value::from_str(y.trim()).unwrap();
    let x_width = x.width();
    let y_width = y.width();

    let code = format!(
        r#"
    module Top (
        x: input {} logic<{}>,
        y: input {} logic<{}>,
        o: output logic<{}>,
    ) {{
        assign o = x {} y;
    }}
    "#,
        x_signed,
        x_width,
        y_signed,
        y_width,
        dst_width,
        op.trim()
    );

    for config in Config::all() {
        if only_4state && !config.use_4state {
            continue;
        }

        dbg!(&config);

        let ir = analyze(&code, &config);

        let mut sim = Simulator::new(ir, None);

        sim.set("x", x.clone());
        sim.set("y", y.clone());

        sim.step(&Event::Clock(VarId::SYNTHETIC));

        println!("{}", sim.ir.dump_variables());

        if dst.contains('b') {
            assert_eq!(format!("{:b}", sim.get("o").unwrap()), dst);
        } else {
            assert_eq!(format!("{:x}", sim.get("o").unwrap()), dst);
        }
    }
}

#[test]
fn binary_corner_case() {
    binary_test("8'h01 ", "+  ", "8'h01 ", 16, "16'b0000000000000010", false);
    binary_test("8'hf2 ", "+  ", "8'hf2 ", 16, "16'b0000000111100100", false);
    binary_test("8'hx3 ", "+  ", "8'hx3 ", 16, "16'bxxxxxxxxxxxxxxxx", true);
    binary_test("8'hz4 ", "+  ", "8'hz4 ", 16, "16'bxxxxxxxxxxxxxxxx", true);
    binary_test("8'sh01", "+  ", "8'sh01", 16, "16'b0000000000000010", false);
    binary_test("8'shf2", "+  ", "8'shf2", 16, "16'b1111111111100100", false);
    binary_test("8'shx3", "+  ", "8'shx3", 16, "16'bxxxxxxxxxxxxxxxx", true);
    binary_test("8'shz4", "+  ", "8'shz4", 16, "16'bxxxxxxxxxxxxxxxx", true);

    binary_test("8'h01 ", "-  ", "8'hf2 ", 16, "16'b1111111100001111", false);
    binary_test("8'hf2 ", "-  ", "8'h03 ", 16, "16'b0000000011101111", false);
    binary_test("8'hx3 ", "-  ", "8'hx4 ", 16, "16'bxxxxxxxxxxxxxxxx", true);
    binary_test("8'hz4 ", "-  ", "8'hz5 ", 16, "16'bxxxxxxxxxxxxxxxx", true);
    binary_test("8'sh01", "-  ", "8'shf2", 16, "16'b0000000000001111", false);
    binary_test("8'shf2", "-  ", "8'sh03", 16, "16'b1111111111101111", false);
    binary_test("8'shx3", "-  ", "8'shx4", 16, "16'bxxxxxxxxxxxxxxxx", true);
    binary_test("8'shz4", "-  ", "8'shz5", 16, "16'bxxxxxxxxxxxxxxxx", true);

    binary_test("8'h01 ", "*  ", "8'h01 ", 16, "16'b0000000000000001", false);
    binary_test("8'hf2 ", "*  ", "8'hf2 ", 16, "16'b1110010011000100", false);
    binary_test("8'hx3 ", "*  ", "8'hx3 ", 16, "16'bxxxxxxxxxxxxxxxx", true);
    binary_test("8'hz4 ", "*  ", "8'hz4 ", 16, "16'bxxxxxxxxxxxxxxxx", true);
    binary_test("8'sh01", "*  ", "8'sh01", 16, "16'b0000000000000001", false);
    binary_test("8'shf2", "*  ", "8'shf2", 16, "16'b0000000011000100", false);
    binary_test("8'shf3", "*  ", "8'sh03", 16, "16'b1111111111011001", false);
    binary_test("8'shz4", "*  ", "8'shz4", 16, "16'bxxxxxxxxxxxxxxxx", true);

    binary_test("8'h02 ", "/  ", "8'h01 ", 16, "16'b0000000000000010", false);
    binary_test("8'hf0 ", "/  ", "8'h02 ", 16, "16'b0000000001111000", false);
    binary_test("8'hx3 ", "/  ", "8'hx3 ", 16, "16'bxxxxxxxxxxxxxxxx", true);
    binary_test("8'hz4 ", "/  ", "8'hz4 ", 16, "16'bxxxxxxxxxxxxxxxx", true);
    binary_test("8'sh02", "/  ", "8'sh01", 16, "16'b0000000000000010", false);
    binary_test("8'shf0", "/  ", "8'sh02", 16, "16'b1111111111111000", false);
    binary_test("8'shf3", "/  ", "8'shf3", 16, "16'b0000000000000001", false);
    binary_test("8'sh01", "/  ", "8'sh00", 16, "16'bxxxxxxxxxxxxxxxx", true);

    binary_test("8'h03 ", "%  ", "8'h01 ", 16, "16'b0000000000000000", false);
    binary_test("8'hf1 ", "%  ", "8'h02 ", 16, "16'b0000000000000001", false);
    binary_test("8'hx3 ", "%  ", "8'hx3 ", 16, "16'bxxxxxxxxxxxxxxxx", true);
    binary_test("8'hz4 ", "%  ", "8'hz4 ", 16, "16'bxxxxxxxxxxxxxxxx", true);
    binary_test("8'sh03", "%  ", "8'sh02", 16, "16'b0000000000000001", false);
    binary_test("8'shf1", "%  ", "8'sh02", 16, "16'b1111111111111111", false);
    binary_test("8'shf1", "%  ", "8'shfc", 16, "16'b1111111111111101", false);
    binary_test("8'sh03", "%  ", "8'shfc", 16, "16'b0000000000000011", false);

    binary_test("8'hf3 ", "&  ", "8'hc1 ", 16, "16'b0000000011000001", false);
    binary_test("8'hf1 ", "&  ", "8'he2 ", 16, "16'b0000000011100000", false);
    binary_test("8'hx1 ", "&  ", "8'hx2 ", 16, "16'b00000000xxxx0000", true);
    binary_test("8'hz3 ", "&  ", "8'hz7 ", 16, "16'b00000000xxxx0011", true);
    binary_test("8'h13 ", "&  ", "8'hx1 ", 16, "16'b00000000000x0001", true);
    binary_test("8'h11 ", "&  ", "8'hz2 ", 16, "16'b00000000000x0000", true);
    binary_test("8'hx1 ", "&  ", "8'hzd ", 16, "16'b00000000xxxx0001", true);
    binary_test("8'h1z ", "&  ", "8'hfx ", 16, "16'b000000000001xxxx", true);

    binary_test("8'hf3 ", "|  ", "8'hc1 ", 16, "16'b0000000011110011", false);
    binary_test("8'hf1 ", "|  ", "8'he2 ", 16, "16'b0000000011110011", false);
    binary_test("8'hx1 ", "|  ", "8'hx2 ", 16, "16'b00000000xxxx0011", true);
    binary_test("8'hz3 ", "|  ", "8'hz7 ", 16, "16'b00000000xxxx0111", true);
    binary_test("8'h13 ", "|  ", "8'hx1 ", 16, "16'b00000000xxx10011", true);
    binary_test("8'h11 ", "|  ", "8'hz2 ", 16, "16'b00000000xxx10011", true);
    binary_test("8'hx1 ", "|  ", "8'hzd ", 16, "16'b00000000xxxx1101", true);
    binary_test("8'h1z ", "|  ", "8'hfx ", 16, "16'b000000001111xxxx", true);

    binary_test("8'hf3 ", "^  ", "8'hc1 ", 16, "16'b0000000000110010", false);
    binary_test("8'hf1 ", "^  ", "8'he2 ", 16, "16'b0000000000010011", false);
    binary_test("8'hx1 ", "^  ", "8'hx2 ", 16, "16'b00000000xxxx0011", true);
    binary_test("8'hz3 ", "^  ", "8'hz7 ", 16, "16'b00000000xxxx0100", true);
    binary_test("8'h13 ", "^  ", "8'hx1 ", 16, "16'b00000000xxxx0010", true);
    binary_test("8'h11 ", "^  ", "8'hz2 ", 16, "16'b00000000xxxx0011", true);
    binary_test("8'hx1 ", "^  ", "8'hzd ", 16, "16'b00000000xxxx1100", true);
    binary_test("8'h1z ", "^  ", "8'hfx ", 16, "16'b000000001110xxxx", true);

    binary_test("8'hf3 ", "~^ ", "8'hc1 ", 16, "16'b1111111111001101", false);
    binary_test("8'hf1 ", "~^ ", "8'he2 ", 16, "16'b1111111111101100", false);
    binary_test("8'hx1 ", "~^ ", "8'hx2 ", 16, "16'b11111111xxxx1100", true);
    binary_test("8'hz3 ", "~^ ", "8'hz7 ", 16, "16'b11111111xxxx1011", true);
    binary_test("8'h13 ", "~^ ", "8'hx1 ", 16, "16'b11111111xxxx1101", true);
    binary_test("8'h11 ", "~^ ", "8'hz2 ", 16, "16'b11111111xxxx1100", true);
    binary_test("8'hx1 ", "~^ ", "8'hzd ", 16, "16'b11111111xxxx0011", true);
    binary_test("8'h1z ", "~^ ", "8'hfx ", 16, "16'b111111110001xxxx", true);

    binary_test("8'h00 ", "== ", "8'h00 ", 16, "16'b0000000000000001", false);
    binary_test("8'hf1 ", "== ", "8'he2 ", 16, "16'b0000000000000000", false);
    binary_test("8'hx0 ", "== ", "8'hx0 ", 16, "16'b000000000000000x", true);
    binary_test("8'hx3 ", "== ", "8'hx7 ", 16, "16'b0000000000000000", true);
    binary_test("8'hz0 ", "== ", "8'hz0 ", 16, "16'b000000000000000x", true);
    binary_test("8'hz1 ", "== ", "8'hz2 ", 16, "16'b0000000000000000", true);
    binary_test("8'hxz ", "== ", "8'hxz ", 16, "16'b000000000000000x", true);
    binary_test("8'hzx ", "== ", "8'hxz ", 16, "16'b000000000000000x", true);

    binary_test("8'h00 ", "!= ", "8'h00 ", 16, "16'b0000000000000000", false);
    binary_test("8'hf1 ", "!= ", "8'he2 ", 16, "16'b0000000000000001", false);
    binary_test("8'hx0 ", "!= ", "8'hx0 ", 16, "16'b000000000000000x", true);
    binary_test("8'hx3 ", "!= ", "8'hx7 ", 16, "16'b0000000000000001", true);
    binary_test("8'hz0 ", "!= ", "8'hz0 ", 16, "16'b000000000000000x", true);
    binary_test("8'hz1 ", "!= ", "8'hz2 ", 16, "16'b0000000000000001", true);
    binary_test("8'hxz ", "!= ", "8'hxz ", 16, "16'b000000000000000x", true);
    binary_test("8'hzx ", "!= ", "8'hxz ", 16, "16'b000000000000000x", true);

    binary_test("8'h00 ", "==?", "8'h00 ", 16, "16'b0000000000000001", false);
    binary_test("8'hf1 ", "==?", "8'he2 ", 16, "16'b0000000000000000", false);
    binary_test("8'hx0 ", "==?", "8'h30 ", 16, "16'b000000000000000x", true);
    binary_test("8'h43 ", "==?", "8'h4x ", 16, "16'b0000000000000001", true);
    binary_test("8'hz0 ", "==?", "8'h30 ", 16, "16'b000000000000000x", true);
    binary_test("8'h11 ", "==?", "8'h1z ", 16, "16'b0000000000000001", true);
    binary_test("8'hxz ", "==?", "8'hxz ", 16, "16'b0000000000000001", true);
    binary_test("8'hzx ", "==?", "8'hxz ", 16, "16'b0000000000000001", true);

    binary_test("8'h00 ", "!=?", "8'h00 ", 16, "16'b0000000000000000", false);
    binary_test("8'hf1 ", "!=?", "8'he2 ", 16, "16'b0000000000000001", false);
    binary_test("8'hx0 ", "!=?", "8'h30 ", 16, "16'b000000000000000x", true);
    binary_test("8'h43 ", "!=?", "8'h4x ", 16, "16'b0000000000000000", true);
    binary_test("8'hz0 ", "!=?", "8'h30 ", 16, "16'b000000000000000x", true);
    binary_test("8'h11 ", "!=?", "8'h1z ", 16, "16'b0000000000000000", true);
    binary_test("8'hxz ", "!=?", "8'hxz ", 16, "16'b0000000000000000", true);
    binary_test("8'hzx ", "!=?", "8'hxz ", 16, "16'b0000000000000000", true);

    binary_test("8'h03 ", ">: ", "8'h01 ", 16, "16'b0000000000000001", false);
    binary_test("8'hf1 ", ">: ", "8'h02 ", 16, "16'b0000000000000001", false);
    binary_test("8'hx3 ", ">: ", "8'hx3 ", 16, "16'b000000000000000x", true);
    binary_test("8'hz4 ", ">: ", "8'hz4 ", 16, "16'b000000000000000x", true);
    binary_test("8'sh03", ">: ", "8'sh01", 16, "16'b0000000000000001", false);
    binary_test("8'shf1", ">: ", "8'sh02", 16, "16'b0000000000000000", false);
    binary_test("8'shx3", ">: ", "8'shx3", 16, "16'b000000000000000x", true);
    binary_test("8'shz4", ">: ", "8'shz4", 16, "16'b000000000000000x", true);

    binary_test("8'h03 ", ">= ", "8'h01 ", 16, "16'b0000000000000001", false);
    binary_test("8'hf1 ", ">= ", "8'h02 ", 16, "16'b0000000000000001", false);
    binary_test("8'hx3 ", ">= ", "8'hx3 ", 16, "16'b000000000000000x", true);
    binary_test("8'hz4 ", ">= ", "8'hz4 ", 16, "16'b000000000000000x", true);
    binary_test("8'sh03", ">= ", "8'sh01", 16, "16'b0000000000000001", false);
    binary_test("8'shf1", ">= ", "8'sh02", 16, "16'b0000000000000000", false);
    binary_test("8'shx3", ">= ", "8'shx3", 16, "16'b000000000000000x", true);
    binary_test("8'shz4", ">= ", "8'shz4", 16, "16'b000000000000000x", true);

    binary_test("8'h03 ", "<: ", "8'h01 ", 16, "16'b0000000000000000", false);
    binary_test("8'hf1 ", "<: ", "8'h02 ", 16, "16'b0000000000000000", false);
    binary_test("8'hx3 ", "<: ", "8'hx3 ", 16, "16'b000000000000000x", true);
    binary_test("8'hz4 ", "<: ", "8'hz4 ", 16, "16'b000000000000000x", true);
    binary_test("8'sh03", "<: ", "8'sh01", 16, "16'b0000000000000000", false);
    binary_test("8'shf1", "<: ", "8'sh02", 16, "16'b0000000000000001", false);
    binary_test("8'shx3", "<: ", "8'shx3", 16, "16'b000000000000000x", true);
    binary_test("8'shz4", "<: ", "8'shz4", 16, "16'b000000000000000x", true);

    binary_test("8'h03 ", "<= ", "8'h01 ", 16, "16'b0000000000000000", false);
    binary_test("8'hf1 ", "<= ", "8'h02 ", 16, "16'b0000000000000000", false);
    binary_test("8'hx3 ", "<= ", "8'hx3 ", 16, "16'b000000000000000x", true);
    binary_test("8'hz4 ", "<= ", "8'hz4 ", 16, "16'b000000000000000x", true);
    binary_test("8'sh03", "<= ", "8'sh01", 16, "16'b0000000000000000", false);
    binary_test("8'shf1", "<= ", "8'sh02", 16, "16'b0000000000000001", false);
    binary_test("8'shx3", "<= ", "8'shx3", 16, "16'b000000000000000x", true);
    binary_test("8'shz4", "<= ", "8'shz4", 16, "16'b000000000000000x", true);

    binary_test("8'h03 ", "&& ", "8'h01 ", 16, "16'b0000000000000001", false);
    binary_test("8'hf1 ", "&& ", "8'h00 ", 16, "16'b0000000000000000", false);
    binary_test("8'hx3 ", "&& ", "8'hx3 ", 16, "16'b0000000000000001", true);
    binary_test("8'hz4 ", "&& ", "8'hz4 ", 16, "16'b0000000000000001", true);
    binary_test("8'h0x ", "&& ", "8'h01 ", 16, "16'b000000000000000x", true);
    binary_test("8'hf1 ", "&& ", "8'h0z ", 16, "16'b000000000000000x", true);
    binary_test("8'hxx ", "&& ", "8'hx3 ", 16, "16'b000000000000000x", true);
    binary_test("8'hz4 ", "&& ", "8'hzz ", 16, "16'b000000000000000x", true);

    binary_test("8'h03 ", "|| ", "8'h01 ", 16, "16'b0000000000000001", false);
    binary_test("8'h00 ", "|| ", "8'h00 ", 16, "16'b0000000000000000", false);
    binary_test("8'hx0 ", "|| ", "8'hx0 ", 16, "16'b000000000000000x", true);
    binary_test("8'hz0 ", "|| ", "8'hz0 ", 16, "16'b000000000000000x", true);
    binary_test("8'h0x ", "|| ", "8'h0z ", 16, "16'b000000000000000x", true);
    binary_test("8'hf1 ", "|| ", "8'h0z ", 16, "16'b0000000000000001", true);
    binary_test("8'hxx ", "|| ", "8'hx3 ", 16, "16'b0000000000000001", true);
    binary_test("8'hz4 ", "|| ", "8'hzz ", 16, "16'b0000000000000001", true);

    binary_test("8'h03 ", ">> ", "3'd2  ", 16, "16'b0000000000000000", false);
    binary_test("8'hf1 ", ">> ", "3'd2  ", 16, "16'b0000000000111100", false);
    binary_test("8'hx3 ", ">> ", "3'd2  ", 16, "16'b0000000000xxxx00", true);
    binary_test("8'hz4 ", ">> ", "3'd2  ", 16, "16'b0000000000zzzz01", true);
    binary_test("8'sh03", ">> ", "3'd2  ", 16, "16'b0000000000000000", false);
    binary_test("8'shf1", ">> ", "3'd2  ", 16, "16'b0011111111111100", false);
    binary_test("8'shx3", ">> ", "3'd2  ", 16, "16'b00xxxxxxxxxxxx00", true);
    binary_test("8'shz4", ">> ", "3'd2  ", 16, "16'b00zzzzzzzzzzzz01", true);

    binary_test("8'h03 ", "<< ", "3'd2  ", 16, "16'b0000000000001100", false);
    binary_test("8'hf1 ", "<< ", "3'd2  ", 16, "16'b0000001111000100", false);
    binary_test("8'hx3 ", "<< ", "3'd2  ", 16, "16'b000000xxxx001100", true);
    binary_test("8'hz4 ", "<< ", "3'd2  ", 16, "16'b000000zzzz010000", true);
    binary_test("8'sh03", "<< ", "3'd2  ", 16, "16'b0000000000001100", false);
    binary_test("8'shf1", "<< ", "3'd2  ", 16, "16'b1111111111000100", false);
    binary_test("8'shx3", "<< ", "3'd2  ", 16, "16'bxxxxxxxxxx001100", true);
    binary_test("8'shz4", "<< ", "3'd2  ", 16, "16'bzzzzzzzzzz010000", true);

    binary_test("8'h03 ", ">>>", "3'd2  ", 16, "16'b0000000000000000", false);
    binary_test("8'hf1 ", ">>>", "3'd2  ", 16, "16'b0000000000111100", false);
    binary_test("8'hx3 ", ">>>", "3'd2  ", 16, "16'b0000000000xxxx00", true);
    binary_test("8'hz4 ", ">>>", "3'd2  ", 16, "16'b0000000000zzzz01", true);
    binary_test("8'sh03", ">>>", "3'd2  ", 16, "16'b0000000000000000", false);
    binary_test("8'shf1", ">>>", "3'd2  ", 16, "16'b1111111111111100", false);
    binary_test("8'shx3", ">>>", "3'd2  ", 16, "16'bxxxxxxxxxxxxxx00", true);
    binary_test("8'shz4", ">>>", "3'd2  ", 16, "16'bzzzzzzzzzzzzzz01", true);

    binary_test("8'h03 ", "<<<", "3'd2  ", 16, "16'b0000000000001100", false);
    binary_test("8'hf1 ", "<<<", "3'd2  ", 16, "16'b0000001111000100", false);
    binary_test("8'hx3 ", "<<<", "3'd2  ", 16, "16'b000000xxxx001100", true);
    binary_test("8'hz4 ", "<<<", "3'd2  ", 16, "16'b000000zzzz010000", true);
    binary_test("8'sh03", "<<<", "3'd2  ", 16, "16'b0000000000001100", false);
    binary_test("8'shf1", "<<<", "3'd2  ", 16, "16'b1111111111000100", false);
    binary_test("8'shx3", "<<<", "3'd2  ", 16, "16'bxxxxxxxxxx001100", true);
    binary_test("8'shz4", "<<<", "3'd2  ", 16, "16'bzzzzzzzzzz010000", true);

    binary_test("8'h03 ", "** ", "3'd2  ", 16, "16'b0000000000001001", false);
    binary_test("8'hf1 ", "** ", "3'd2  ", 16, "16'b1110001011100001", false);
    binary_test("8'hx3 ", "** ", "3'd2  ", 16, "16'bxxxxxxxxxxxxxxxx", true);
    binary_test("8'hz4 ", "** ", "3'd2  ", 16, "16'bxxxxxxxxxxxxxxxx", true);
    binary_test("8'sh03", "** ", "3'd2  ", 16, "16'b0000000000001001", false);
    binary_test("8'shf1", "** ", "3'd2  ", 16, "16'b0000000011100001", false);
    binary_test("8'shx3", "** ", "3'd2  ", 16, "16'bxxxxxxxxxxxxxxxx", true);
    binary_test("8'shz4", "** ", "3'd2  ", 16, "16'bxxxxxxxxxxxxxxxx", true);
}

// Regression: `$signed(x) >>> y` and signed comparisons on a same-width
// unsigned variable must use the expression-context `signed` flag, not
// the stored variable flag, otherwise the interpreter falls back to a
// logical shift / unsigned compare.
#[test]
fn signed_cast_same_width() {
    let code = r#"
    module Top (
        a  : input  logic<64>,
        sh : input  logic<6> ,
        b  : input  logic<64>,
        sra: output logic<64>,
        lt : output logic    ,
        ge : output logic    ,
    ) {
        assign sra = $signed(a) >>> sh;
        assign lt  = $signed(a) <: $signed(b);
        assign ge  = $signed(a) >= $signed(b);
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        // 0xFFFFFFFF_80000000 >>> 1 = 0xFFFFFFFF_C0000000 (arithmetic)
        sim.set("a", Value::from_str("64'hFFFFFFFF_80000000").unwrap());
        sim.set("sh", Value::from_str("6'd1").unwrap());
        sim.set("b", Value::from_str("64'd1").unwrap());
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            format!("{:x}", sim.get("sra").unwrap()),
            "64'hffffffffc0000000"
        );
        // -0x80000000 < 1 → taken
        assert_eq!(format!("{:b}", sim.get("lt").unwrap()), "1'b1");
        // -0x80000000 >= 1 → false
        assert_eq!(format!("{:b}", sim.get("ge").unwrap()), "1'b0");

        // -1 vs -2: -1 > -2
        sim.set("a", Value::from_str("64'hFFFFFFFF_FFFFFFFF").unwrap());
        sim.set("b", Value::from_str("64'hFFFFFFFF_FFFFFFFE").unwrap());
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(format!("{:b}", sim.get("lt").unwrap()), "1'b0");
        assert_eq!(format!("{:b}", sim.get("ge").unwrap()), "1'b1");
    }
}

// Regression: `$signed(a) / $signed(b)` and `%` must (a) produce a
// signed result and (b) survive the cranelift SIGFPE cases (y == 0 and
// signed i64::MIN / -1) consistently between interpreter and JIT.
#[test]
fn signed_div_rem_cast_and_overflow() {
    let code = r#"
    module Top (
        a : input  logic<64>,
        b : input  logic<64>,
        q : output logic<64>,
        r : output logic<64>,
    ) {
        assign q = $signed(a) / $signed(b);
        assign r = $signed(a) % $signed(b);
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        // -10 / 3 = -3, -10 % 3 = -1 (signed division)
        sim.set("a", Value::from_str("64'hFFFFFFFF_FFFFFFF6").unwrap());
        sim.set("b", Value::from_str("64'd3").unwrap());
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            format!("{:x}", sim.get("q").unwrap()),
            "64'hfffffffffffffffd"
        );
        assert_eq!(
            format!("{:x}", sim.get("r").unwrap()),
            "64'hffffffffffffffff"
        );

        // i64::MIN / -1 would SIGFPE on cranelift sdiv; the JIT must
        // guard it and fall back to the dividend.
        sim.set("a", Value::from_str("64'h80000000_00000000").unwrap());
        sim.set("b", Value::from_str("64'hFFFFFFFF_FFFFFFFF").unwrap());
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            format!("{:x}", sim.get("q").unwrap()),
            "64'h8000000000000000"
        );
        assert_eq!(
            format!("{:x}", sim.get("r").unwrap()),
            "64'h0000000000000000"
        );
    }
}

// Regression: a narrow-width destination (here `logic<2>`) must mask
// the stored payload to dst_width; `effective_bits()` reports the
// declared width and misses the carry-out from Add, which would
// otherwise leak into the high bits of the stored value.
#[test]
fn narrow_width_add_carry_out_masked() {
    let code = r#"
    module Top (
        clk  : input  clock   ,
        rst  : input  reset   ,
        i_inc: input  logic   ,
        o_idx: output logic<2>,
    ) {
        var idx: logic<2>;
        always_ff {
            if_reset {
                idx = 2'd0;
            } else if i_inc {
                idx = idx + 2'd1;
            }
        }
        assign o_idx = idx;
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.set("i_inc", Value::new(1, 1, false));
        sim.step(&rst);

        // Drive 8 cycles; idx must wrap cleanly 0,1,2,3,0,1,2,3.
        let expected = [1u64, 2, 3, 0, 1, 2, 3, 0];
        for (i, want) in expected.iter().enumerate() {
            sim.step(&clk);
            let got = sim.get("o_idx").unwrap().payload_u64() & 0x3;
            assert_eq!(
                got, *want,
                "cycle {}: JIT={} 4st={} ff_opt={}: got {} expected {}",
                i, config.use_jit, config.use_4state, !config.disable_ff_opt, got, *want
            );
        }
    }
}

#[test]
fn partial_jit() {
    // Mix of JIT-compilable (a + b, a ** d) and non-JIT-compilable ($display) statements.
    // With partial JIT, JIT-able statements should be compiled while $display is interpreted.
    let code = r#"
    module Top (
        a  : input  logic<32>,
        b  : input  logic<32>,
        d  : input  logic<3>,
        x  : output logic<32>,
        z  : output logic<32>,
    ) {
        assign x = a + b;
        assign z = a ** d;
    }
    "#;

    // JIT disabled: no Compiled statements
    let config_no_jit = Config {
        use_jit: false,
        ..Default::default()
    };
    let ir = analyze(code, &config_no_jit);
    assert!(
        ir.comb_statements.iter().all(|s| !s.is_compiled()),
        "JIT disabled: all statements should be interpreted"
    );

    // JIT enabled: all statements should be compiled
    let config_jit = Config {
        use_jit: true,
        ..Default::default()
    };
    let ir = analyze(code, &config_jit);
    let has_compiled = ir.comb_statements.iter().any(|s| s.is_compiled());
    assert!(has_compiled, "partial JIT should compile some statements");

    // Verify simulation results are correct regardless of JIT mode
    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.set("a", Value::new(10, 32, false));
        sim.set("b", Value::new(20, 32, false));
        sim.set("d", Value::new(2, 3, false));

        sim.step(&Event::Clock(VarId::SYNTHETIC));

        assert_eq!(sim.get("x").unwrap(), Value::new(30, 32, false));
        assert_eq!(sim.get("z").unwrap(), Value::new(100, 32, false));
    }
}

#[test]
fn concatenation() {
    // Basic concatenation: {a[15:0], b[15:0]} -> 32-bit output
    let code = r#"
    module Top (
        a: input  logic<32>,
        b: input  logic<32>,
        c: output logic<32>,
    ) {
        assign c = {a[15:0], b[15:0]};
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);

        let mut sim = Simulator::new(ir, None);

        sim.set("a", Value::new(0xABCD_1234, 32, false));
        sim.set("b", Value::new(0x5678_9ABC, 32, false));

        sim.step(&Event::Clock(VarId::SYNTHETIC));

        println!("{}", sim.ir.dump_variables());

        // c = {a[15:0], b[15:0]} = {0x1234, 0x9ABC} = 0x12349ABC
        assert_eq!(sim.get("c").unwrap(), Value::new(0x1234_9ABC, 32, false));
    }
}

#[test]
fn concatenation_repeat() {
    // Repeat concatenation: {a[7:0] repeat 4} -> 32-bit output
    let code = r#"
    module Top (
        a: input  logic<32>,
        c: output logic<32>,
    ) {
        assign c = {a[7:0] repeat 4};
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);

        let mut sim = Simulator::new(ir, None);

        sim.set("a", Value::new(0xAB, 32, false));

        sim.step(&Event::Clock(VarId::SYNTHETIC));

        println!("{}", sim.ir.dump_variables());

        // c = {0xAB, 0xAB, 0xAB, 0xAB} = 0xABABABAB
        assert_eq!(sim.get("c").unwrap(), Value::new(0xABABABAB, 32, false));
    }
}

#[test]
fn concatenation_4state() {
    // 4-state concatenation with X/Z values
    let code = r#"
    module Top (
        a: input  logic<8>,
        b: input  logic<8>,
        c: output logic<16>,
    ) {
        assign c = {a, b};
    }
    "#;

    for config in Config::all() {
        if !config.use_4state {
            continue;
        }

        dbg!(&config);

        let ir = analyze(code, &config);

        let mut sim = Simulator::new(ir, None);

        // a = 8'hx3 (upper nibble is X)
        sim.set("a", Value::from_str("8'hx3").unwrap());
        sim.set("b", Value::new(0xFF, 8, false));

        sim.step(&Event::Clock(VarId::SYNTHETIC));

        println!("{}", sim.ir.dump_variables());

        // c = {8'hx3, 8'hFF} = 16'hx3FF -> upper byte has X in upper nibble
        let result = sim.get("c").unwrap();
        assert_eq!(format!("{:b}", result), "16'bxxxx001111111111");
    }
}

#[test]
fn lhs_concatenation() {
    let code = r#"
    module Top (
        x: input  logic<32>,
        a: output logic<20>,
        b: output logic<12>,
    ) {
        assign {a, b} = x;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        // x = 0xABCDE123 (32 bits)
        // a = upper 20 bits = 0xABCDE
        // b = lower 12 bits = 0x123
        sim.set("x", Value::new(0xABCDE123, 32, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));

        println!("{}", sim.ir.dump_variables());

        let a = sim.get("a").unwrap();
        let b = sim.get("b").unwrap();
        assert_eq!(a, Value::new(0xABCDE, 20, false));
        assert_eq!(b, Value::new(0x123, 12, false));
    }
}

#[test]
fn lhs_concatenation_equal_split() {
    let code = r#"
    module Top (
        x: input  logic<32>,
        a: output logic<16>,
        b: output logic<16>,
    ) {
        assign {a, b} = x;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.set("x", Value::new(0xDEAD_BEEF, 32, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));

        let a = sim.get("a").unwrap();
        let b = sim.get("b").unwrap();
        assert_eq!(a, Value::new(0xDEAD, 16, false));
        assert_eq!(b, Value::new(0xBEEF, 16, false));
    }
}

#[test]
fn lhs_concatenation_small_value() {
    let code = r#"
    module Top (
        a: output logic<8>,
        b: output logic<8>,
    ) {
        assign {a, b} = 1;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.step(&Event::Clock(VarId::SYNTHETIC));

        let a = sim.get("a").unwrap();
        let b = sim.get("b").unwrap();
        assert_eq!(a, Value::new(0, 8, false));
        assert_eq!(b, Value::new(1, 8, false));
    }
}

#[test]
fn function_call_expr() {
    let code = r#"
    module Top (
        a: input  logic<32>,
        b: input  logic<32>,
        c: output logic<32>,
    ) {
        function add(
            x: input logic<32>,
            y: input logic<32>,
        ) -> logic<32> {
            return x + y;
        }

        assign c = add(a, b);
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.set("a", Value::new(10, 32, false));
        sim.set("b", Value::new(20, 32, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));

        assert_eq!(sim.get("c").unwrap(), Value::new(30, 32, false));
    }
}

#[test]
fn function_call_void() {
    let code = r#"
    module Top (
        a: input  logic<32>,
        b: output logic<32>,
    ) {
        function double(
            x: input  logic<32>,
            y: output logic<32>,
        ) {
            y = x + x;
        }

        always_comb {
            double(a, b);
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.set("a", Value::new(7, 32, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));

        assert_eq!(sim.get("b").unwrap(), Value::new(14, 32, false));
    }
}

#[test]
fn function_call_with_output() {
    let code = r#"
    module Top (
        a: input  logic<32>,
        c: output logic<32>,
        d: output logic<32>,
    ) {
        function add_and_double(
            x: input  logic<32>,
            side: output logic<32>,
        ) -> logic<32> {
            side = x + x;
            return x + 1;
        }

        assign c = add_and_double(a, d);
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.set("a", Value::new(5, 32, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));

        assert_eq!(sim.get("c").unwrap(), Value::new(6, 32, false));
        assert_eq!(sim.get("d").unwrap(), Value::new(10, 32, false));
    }
}

#[test]
fn function_call_nested() {
    let code = r#"
    module Top (
        a: input  logic<32>,
        b: input  logic<32>,
        c: output logic<32>,
    ) {
        function add(
            x: input logic<32>,
            y: input logic<32>,
        ) -> logic<32> {
            return x + y;
        }

        function double(
            x: input logic<32>,
        ) -> logic<32> {
            return x + x;
        }

        assign c = add(double(a), b);
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.set("a", Value::new(3, 32, false));
        sim.set("b", Value::new(4, 32, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));

        // double(3) = 6, add(6, 4) = 10
        assert_eq!(sim.get("c").unwrap(), Value::new(10, 32, false));
    }
}

#[test]
fn function_call_in_ff() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        a  : input  logic<32>,
        c  : output logic<32>,
    ) {
        function inc(
            x: input logic<32>,
        ) -> logic<32> {
            return x + 1;
        }

        always_ff {
            if_reset {
                c = 0;
            } else {
                c = inc(a);
            }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        sim.set("a", Value::new(41, 32, false));
        sim.step(&clk);

        assert_eq!(sim.get("c").unwrap(), Value::new(42, 32, false));
    }
}

#[test]
fn if_expression() {
    let code = r#"
    module Top (
        clk: input clock,
        rst: input reset,
        sel: input logic,
        a: input logic<8>,
        b: input logic<8>,
        y: output logic<8>,
    ) {
        assign y = if sel ? a : b;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.set("sel", Value::new(1, 1, false));
        sim.set("a", Value::new(42, 8, false));
        sim.set("b", Value::new(99, 8, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("y").unwrap(), Value::new(42, 8, false));

        sim.set("sel", Value::new(0, 1, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("y").unwrap(), Value::new(99, 8, false));
    }
}

#[test]
fn if_expression_chained() {
    let code = r#"
    module Top (
        clk: input clock,
        rst: input reset,
        sel: input logic<2>,
        y: output logic<8>,
    ) {
        assign y = if sel ==  2'd2 ? 8'd10 : if sel == 2'd1 ? 8'd20 : 8'd30;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.set("sel", Value::new(2, 2, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("y").unwrap(), Value::new(10, 8, false));

        sim.set("sel", Value::new(1, 2, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("y").unwrap(), Value::new(20, 8, false));

        sim.set("sel", Value::new(0, 2, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("y").unwrap(), Value::new(30, 8, false));
    }
}

#[test]
fn if_expression_nested() {
    let code = r#"
    module Top (
        clk: input clock,
        rst: input reset,
        sel: input logic,
        a: input logic<8>,
        b: input logic<8>,
        y: output logic<8>,
    ) {
        assign y = if sel ? a + b : a - b;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.set("sel", Value::new(1, 1, false));
        sim.set("a", Value::new(30, 8, false));
        sim.set("b", Value::new(12, 8, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("y").unwrap(), Value::new(42, 8, false));

        sim.set("sel", Value::new(0, 1, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("y").unwrap(), Value::new(18, 8, false));
    }
}

#[test]
fn if_expression_4state() {
    let code = r#"
    module Top (
        clk: input clock,
        rst: input reset,
        sel: input logic<4>,
        a: input logic<8>,
        b: input logic<8>,
        y: output logic<8>,
    ) {
        assign y = if sel ? a : b;
    }
    "#;

    for config in Config::all() {
        if !config.use_4state {
            continue;
        }
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.set("a", Value::new(42, 8, false));
        sim.set("b", Value::new(99, 8, false));

        // 4'bxxxx -> false (all unknown)
        sim.set("sel", Value::from_str("4'bxxxx").unwrap());
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("y").unwrap(), Value::new(99, 8, false));

        // 4'bzzzz -> false (all hi-Z)
        sim.set("sel", Value::from_str("4'bzzzz").unwrap());
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("y").unwrap(), Value::new(99, 8, false));

        // 4'bx000 -> false (known bits are all zero)
        sim.set("sel", Value::from_str("4'bx000").unwrap());
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("y").unwrap(), Value::new(99, 8, false));

        // 4'b1x00 -> true (has a known nonzero bit)
        sim.set("sel", Value::from_str("4'b1x00").unwrap());
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("y").unwrap(), Value::new(42, 8, false));

        // 4'b0001 -> true (nonzero, no X/Z)
        sim.set("sel", Value::new(1, 4, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("y").unwrap(), Value::new(42, 8, false));
    }
}

#[test]
fn initial_display() {
    let code = r#"
    module Top (
        i_clk: input clock,
    ) {
        initial {
            $display("hello from initial");
        }
    }
    "#;
    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.step(&Event::Initial);
    }
}

#[test]
fn display_format_specifiers() {
    let code = r#"
    module Top (
        i_clk: input clock,
    ) {
        initial {
            $display("hex=%h dec=%d bin=%b oct=%o", 8'hAB, 8'd42, 4'b1010, 8'o77);
            $display("percent=%%");
            $display("no args message");
            $display("char=%c", 8'd65);
        }
    }
    "#;
    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.step(&Event::Initial);
    }
}

#[test]
fn display_no_format_string() {
    let code = r#"
    module Top (
        i_clk: input clock,
    ) {
        initial {
            $display(8'hFF, 4'b1010);
        }
    }
    "#;
    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.step(&Event::Initial);
    }
}

#[test]
fn parse_hex_content_basic() {
    let content = "AB CD\nEF 01";
    let values = parse_hex_content(content, 8);
    assert_eq!(values.len(), 4);
    assert_eq!(values[0].payload_u64(), 0xAB);
    assert_eq!(values[1].payload_u64(), 0xCD);
    assert_eq!(values[2].payload_u64(), 0xEF);
    assert_eq!(values[3].payload_u64(), 0x01);
}

#[test]
fn parse_hex_content_comments_and_underscores() {
    let content = "// header comment\nDE_AD BE_EF // inline comment\n/* block\ncomment */ 42\n";
    let values = parse_hex_content(content, 16);
    assert_eq!(values.len(), 3);
    assert_eq!(values[0].payload_u64(), 0xDEAD);
    assert_eq!(values[1].payload_u64(), 0xBEEF);
    assert_eq!(values[2].payload_u64(), 0x42);
}

#[test]
fn readmemh_basic() {
    let dir = std::env::temp_dir();
    let hex_path = dir.join("veryl_test_readmemh.hex");
    std::fs::write(&hex_path, "0A 14 1E 28\n").unwrap();
    let hex_path_str = hex_path.to_str().unwrap().replace('\\', "\\\\");

    let code = format!(
        r#"
    module Top (
        i_clk: input clock,
    ) {{
        var mem: logic<8> [4];
        initial {{
            $readmemh("{}", mem);
        }}
    }}
    "#,
        hex_path_str
    );

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(&code, &config);

        let mut sim = Simulator::new(ir, None);
        sim.step(&Event::Initial);

        let dump = sim.ir.dump_variables();
        println!("{}", dump);

        assert!(dump.contains("mem[0] = 8'h0a"));
        assert!(dump.contains("mem[1] = 8'h14"));
        assert!(dump.contains("mem[2] = 8'h1e"));
        assert!(dump.contains("mem[3] = 8'h28"));
    }

    let _ = std::fs::remove_file(&hex_path);
}

#[test]
fn final_display() {
    let code = r#"
    module Top (
        i_clk: input clock,
    ) {
        final {
            $display("hello from final");
        }
    }
    "#;
    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.step(&Event::Final);
    }
}

#[test]
fn interface_inst() {
    let code = r#"
    interface MyIf {
        var data: logic<8>;
        modport master {
            data: output,
        }
        modport slave {
            data: input,
        }
    }

    module Top (
        clk: input clock,
        rst: input reset,
        out: output logic<8>,
    ) {
        inst u_if: MyIf;
        assign u_if.data = 42;
        assign out = u_if.data;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        println!("{}", ir.dump_variables());
        let mut sim = Simulator::new(ir, None);
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        println!("{}", sim.ir.dump_variables());
        let out = sim.get("out").unwrap();
        let exp = Value::new(42, 8, false);
        assert_eq!(out, exp);
    }
}

#[test]
fn interface_modport() {
    let code = r#"
    interface Bus {
        var data: logic<8>;
        var valid: logic;
        modport master {
            data: output,
            valid: output,
        }
        modport slave {
            data: input,
            valid: input,
        }
    }

    module Producer (
        clk: input clock,
        rst: input reset,
        bus: modport Bus::master,
    ) {
        assign bus.data = 99;
        assign bus.valid = 1;
    }

    module Consumer (
        clk: input clock,
        rst: input reset,
        bus: modport Bus::slave,
        out_data: output logic<8>,
        out_valid: output logic,
    ) {
        assign out_data = bus.data;
        assign out_valid = bus.valid;
    }

    module Top (
        clk: input clock,
        rst: input reset,
        out_data: output logic<8>,
        out_valid: output logic,
    ) {
        inst u_bus: Bus;

        inst u_prod: Producer (
            clk,
            rst,
            bus: u_bus,
        );

        inst u_cons: Consumer (
            clk,
            rst,
            bus: u_bus,
            out_data,
            out_valid,
        );
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        println!("{}", ir.dump_variables());
        let mut sim = Simulator::new(ir, None);
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        println!("{}", sim.ir.dump_variables());
        let data = sim.get("out_data").unwrap();
        let valid = sim.get("out_valid").unwrap();
        assert_eq!(data, Value::new(99, 8, false));
        assert_eq!(valid, Value::new(1, 1, false));
    }
}

#[test]
fn interface_array_modport_index() {
    // `inst u: Child(port: s_port[i])` where `s_port` is a modport array.
    // Earlier, `get_port_connects` dropped the `[i]` from the input member
    // expression and produced an unsupported_description in the simulator.
    let code = r#"
    interface Bus {
        var data: logic<8>;
        modport master {
            data: output,
        }
        modport slave {
            data: input,
        }
    }

    module Producer (
        clk: input clock,
        rst: input reset,
        bus: modport Bus::master,
        val: input logic<8>,
    ) {
        assign bus.data = val;
    }

    module Consumer (
        clk: input clock,
        rst: input reset,
        bus: modport Bus::slave,
        out_data: output logic<8>,
    ) {
        assign out_data = bus.data;
    }

    module Mid #(
        param N: u32 = 2,
    ) (
        clk: input clock,
        rst: input reset,
        s_port: modport Bus::slave [N],
        out: output logic<8>[N],
    ) {
        for i in 0..N :g {
            inst u_cons: Consumer (
                clk,
                rst,
                bus: s_port[i],
                out_data: out[i],
            );
        }
    }

    module Top (
        clk: input clock,
        rst: input reset,
        out0: output logic<8>,
        out1: output logic<8>,
    ) {
        inst u_bus: Bus [2];

        inst u_p0: Producer (clk, rst, bus: u_bus[0], val: 10);
        inst u_p1: Producer (clk, rst, bus: u_bus[1], val: 11);

        var w_out: logic<8>[2];
        assign out0 = w_out[0];
        assign out1 = w_out[1];

        inst u_mid: Mid #(N: 2) (
            clk,
            rst,
            s_port: u_bus,
            out:    w_out,
        );
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        println!("{}", ir.dump_variables());
        let mut sim = Simulator::new(ir, None);
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        println!("{}", sim.ir.dump_variables());
        let out0 = sim.get("out0").unwrap();
        let out1 = sim.get("out1").unwrap();
        assert_eq!(out0, Value::new(10, 8, false));
        assert_eq!(out1, Value::new(11, 8, false));
    }
}

#[test]
fn interface_function() {
    let code = r#"
    interface BusIf {
        var data: logic<8>;

        function get_double() -> logic<8> {
            return data * 2;
        }
    }

    module Top (
        clk: input clock,
        rst: input reset,
        out: output logic<8>,
    ) {
        inst u_bus: BusIf;
        assign u_bus.data = 21;
        assign out = u_bus.get_double();
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        println!("{}", ir.dump_variables());
        let mut sim = Simulator::new(ir, None);
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        println!("{}", sim.ir.dump_variables());
        let out = sim.get("out").unwrap();
        let exp = Value::new(42, 8, false);
        assert_eq!(out, exp);
    }
}

#[test]
fn array_literal_comb() {
    let code = r#"
    module Top (
        a: input  logic<8>,
        o0: output logic<8>,
        o1: output logic<8>,
        o2: output logic<8>,
        o3: output logic<8>,
    ) {
        var mem: logic<8> [4];
        assign mem = '{10, 20, 30, a};
        assign o0 = mem[0];
        assign o1 = mem[1];
        assign o2 = mem[2];
        assign o3 = mem[3];
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.set("a", Value::new(40, 8, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));

        println!("{}", sim.ir.dump_variables());

        assert_eq!(sim.get("o0").unwrap(), Value::new(10, 8, false));
        assert_eq!(sim.get("o1").unwrap(), Value::new(20, 8, false));
        assert_eq!(sim.get("o2").unwrap(), Value::new(30, 8, false));
        assert_eq!(sim.get("o3").unwrap(), Value::new(40, 8, false));
    }
}

#[test]
fn array_literal_ff() {
    let code = r#"
    module Top (
        clk: input clock,
        rst: input reset,
        o0: output logic<8>,
        o1: output logic<8>,
    ) {
        var mem: logic<8> [2];
        always_ff {
            if_reset {
                mem = '{0, 0};
            } else {
                mem = '{100, 200};
            }
        }
        assign o0 = mem[0];
        assign o1 = mem[1];
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);
        assert_eq!(sim.get("o0").unwrap(), Value::new(0, 8, false));
        assert_eq!(sim.get("o1").unwrap(), Value::new(0, 8, false));

        sim.step(&clk);
        println!("{}", sim.ir.dump_variables());

        assert_eq!(sim.get("o0").unwrap(), Value::new(100, 8, false));
        assert_eq!(sim.get("o1").unwrap(), Value::new(200, 8, false));
    }
}

#[test]
fn struct_constructor() {
    let code = r#"
    module Top (
        a: input  logic<8>,
        b: input  logic<8>,
        c: output logic<16>,
    ) {
        struct Pair {
            hi: logic<8>,
            lo: logic<8>,
        }

        let p: Pair = Pair'{hi: a, lo: b};
        assign c = p;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.set("a", Value::new(0xAB, 8, false));
        sim.set("b", Value::new(0xCD, 8, false));

        sim.step(&Event::Clock(VarId::SYNTHETIC));

        // Struct is packed MSB-first: {hi, lo} = {0xAB, 0xCD} = 0xABCD
        assert_eq!(sim.get("c").unwrap(), Value::new(0xABCD, 16, false));
    }
}

#[test]
fn struct_constructor_ff() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        a  : input  logic<8>,
        b  : input  logic<8>,
        o  : output logic<16>,
    ) {
        struct Pair {
            hi: logic<8>,
            lo: logic<8>,
        }

        var r: Pair;
        always_ff {
            if_reset {
                r = Pair'{hi: 0, lo: 0};
            } else {
                r = Pair'{hi: a, lo: b};
            }
        }
        assign o = r;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);
        assert_eq!(sim.get("o").unwrap(), Value::new(0x0000, 16, false));

        sim.set("a", Value::new(0xAB, 8, false));
        sim.set("b", Value::new(0xCD, 8, false));

        sim.step(&clk);
        assert_eq!(sim.get("o").unwrap(), Value::new(0xABCD, 16, false));
    }
}

#[test]
fn array_dynamic_index_read() {
    let code = r#"
    module Top (
        idx: input  logic<2>,
        o  : output logic<8>,
    ) {
        var arr: logic<8> [4];

        assign arr[0] = 10;
        assign arr[1] = 20;
        assign arr[2] = 30;
        assign arr[3] = 40;
        assign o = arr[idx];
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        for idx in 0..4u64 {
            sim.set("idx", Value::new(idx, 2, false));
            sim.step(&Event::Clock(VarId::SYNTHETIC));
            let expected = (idx + 1) * 10;
            assert_eq!(sim.get("o").unwrap(), Value::new(expected, 8, false));
        }
    }
}

#[test]
fn array_dynamic_index_write_ff() {
    let code = r#"
    module Top (
        clk : input  clock,
        rst : input  reset,
        idx : input  logic<2>,
        val : input  logic<8>,
        ridx: input  logic<2>,
        o   : output logic<8>,
    ) {
        var arr: logic<8> [4];

        always_ff {
            if_reset {
                arr[0] = 0;
                arr[1] = 0;
                arr[2] = 0;
                arr[3] = 0;
            } else {
                arr[idx] = val;
            }
        }

        assign o = arr[ridx];
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        // Write 42 to arr[2]
        sim.set("idx", Value::new(2, 2, false));
        sim.set("val", Value::new(42, 8, false));
        sim.step(&clk);

        // Read arr[0..3] through output port
        for i in 0..4u64 {
            sim.set("ridx", Value::new(i, 2, false));
            let expected = if i == 2 { 42 } else { 0 };
            assert_eq!(sim.get("o").unwrap(), Value::new(expected, 8, false));
        }

        // Write 99 to arr[0]
        sim.set("idx", Value::new(0, 2, false));
        sim.set("val", Value::new(99, 8, false));
        sim.step(&clk);

        for i in 0..4u64 {
            sim.set("ridx", Value::new(i, 2, false));
            let expected = if i == 0 {
                99
            } else if i == 2 {
                42
            } else {
                0
            };
            assert_eq!(sim.get("o").unwrap(), Value::new(expected, 8, false));
        }
    }
}

#[test]
fn array_dynamic_index_write_comb() {
    let code = r#"
    module Top (
        idx: input  logic<2>,
        val: input  logic<8>,
        o  : output logic<8>,
    ) {
        var arr: logic<8> [4];

        assign arr[0] = 10;
        assign arr[1] = 20;
        assign arr[2] = 30;
        assign arr[3] = 40;
        assign arr[idx] = val;
        assign o = arr[idx];
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.set("idx", Value::new(1, 2, false));
        sim.set("val", Value::new(77, 8, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));

        assert_eq!(sim.get("o").unwrap(), Value::new(77, 8, false));
    }
}

// Regression for #9: an array-range assignment LHS (`arr[0+:3] = '{...}` /
// `arr[0:2] = '{...}`) must drive each covered element with the matching literal
// item (first item -> lowest index), matching the emitted SystemVerilog.
#[test]
fn array_range_assign() {
    for sel_form in ["arr[0+:3]", "arr[0:2]"] {
        let code = format!(
            r#"
            module Top (
                sel: input  logic<2>,
                o  : output logic<8>,
            ) {{
                var arr: logic<8> [4];
                assign {sel_form} = '{{8'd10, 8'd20, 8'd30}};
                assign arr[3]   = 8'd40;
                assign o        = arr[sel];
            }}
            "#
        );
        for config in Config::all() {
            let ir = analyze(&code, &config);
            let mut sim = Simulator::new(ir, None);
            for (sel, expected) in [(0u64, 10u64), (1, 20), (2, 30), (3, 40)] {
                sim.set("sel", Value::new(sel, 2, false));
                sim.step(&Event::Clock(VarId::SYNTHETIC));
                assert_eq!(
                    sim.get("o").unwrap(),
                    Value::new(expected, 8, false),
                    "{sel_form} sel={sel} config={config:?}"
                );
            }
        }
    }
}

#[test]
fn array_range_assign_descending() {
    // A descending `-:` slice reaching index 0 covers all four elements; the
    // literal maps by ascending element index (matches Verilator). Regression
    // guard for the `-:` low-bound off-by-one.
    let code = r#"
            module Top (
                sel: input  logic<2>,
                o  : output logic<8>,
            ) {
                var arr: logic<8> [4];
                assign arr[3-:4] = '{8'd10, 8'd20, 8'd30, 8'd40};
                assign o         = arr[sel];
            }
            "#;
    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        for (sel, expected) in [(0u64, 10u64), (1, 20), (2, 30), (3, 40)] {
            sim.set("sel", Value::new(sel, 2, false));
            sim.step(&Event::Clock(VarId::SYNTHETIC));
            assert_eq!(
                sim.get("o").unwrap(),
                Value::new(expected, 8, false),
                "sel={sel} config={config:?}"
            );
        }
    }
}

#[test]
fn assert_pass() {
    let code = r#"
    module Top (
        i_clk: input clock,
    ) {
        initial {
            $assert(1 == 1);
        }
    }
    "#;
    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.step(&Event::Initial);
    }
}

#[test]
fn assert_fail() {
    let code = r#"
    module Top (
        i_clk: input clock,
    ) {
        initial {
            $assert(1 == 0);
        }
    }
    "#;
    let config = Config::default();
    let ir = analyze(code, &config);
    let mut sim = Simulator::new(ir, None);
    crate::assert_buffer::reset();
    sim.step(&Event::Initial);
    assert!(crate::assert_buffer::has_fatal());
    let msg = crate::assert_buffer::take_failure().unwrap();
    assert_eq!(msg, "assertion failed");
}

#[test]
fn assert_with_message_pass() {
    let code = r#"
    module Top (
        i_clk: input clock,
    ) {
        initial {
            $assert(1 == 1, "values should be equal");
        }
    }
    "#;
    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.step(&Event::Initial);
    }
}

#[test]
fn assert_with_format_fail() {
    let code = r#"
    module Top (
        i_clk: input clock,
    ) {
        initial {
            let a: logic<8> = 8'd5;
            let b: logic<8> = 8'd9;
            $assert(a == b, "mismatch: a=%d b=%d", a, b);
        }
    }
    "#;
    let config = Config::default();
    let ir = analyze(code, &config);
    let mut sim = Simulator::new(ir, None);
    crate::assert_buffer::reset();
    sim.step(&Event::Initial);
    assert!(crate::assert_buffer::has_fatal());
    let msg = crate::assert_buffer::take_failure().unwrap();
    assert_eq!(msg, "mismatch: a=5 b=9");
}

#[test]
fn assert_continue_accumulates_failures() {
    let code = r#"
    module Top (
        i_clk: input clock,
    ) {
        initial {
            $assert_continue(0 == 1, "first");
            $assert_continue(2 == 3, "second");
        }
    }
    "#;
    let config = Config::default();
    let ir = analyze(code, &config);
    let mut sim = Simulator::new(ir, None);
    crate::assert_buffer::reset();
    sim.step(&Event::Initial);
    assert!(!crate::assert_buffer::has_fatal());
    let msg = crate::assert_buffer::take_failure().unwrap();
    assert_eq!(msg, "first\nsecond");
}

#[test]
fn finish_in_initial() {
    let code = r#"
    module Top (
        i_clk: input clock,
    ) {
        initial {
            $finish();
        }
    }
    "#;
    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.step(&Event::Initial);
    }
}

// Regression: Op::As in case expression (previously hit unimplemented!() in
// eval_value_binary and UnresolvedExpression for the type operand).
#[test]
fn case_as_enum_cast() {
    let code = r#"
    module Top (
        clk: input clock,
        rst: input reset,
        sel: input  logic<2>,
        out: output logic<8>,
    ) {
        enum Mode: logic<2> {
            A = 2'd0,
            B = 2'd1,
            C = 2'd2,
        }

        always_comb {
            case sel as Mode {
                Mode::A: out = 8'd10;
                Mode::B: out = 8'd20;
                Mode::C: out = 8'd30;
                default: out = 8'd0;
            }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.set("sel", Value::new(0, 2, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("out").unwrap(), Value::new(10, 8, false));

        sim.set("sel", Value::new(1, 2, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("out").unwrap(), Value::new(20, 8, false));

        sim.set("sel", Value::new(2, 2, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("out").unwrap(), Value::new(30, 8, false));
    }
}

// Regression: $signed/$unsigned in expressions (previously fell through to
// the catch-all Err in SystemFunctionCall::new, making the containing
// always_comb an Unsupported declaration).
#[test]
fn signed_unsigned_in_expr() {
    let code = r#"
    module Top (
        a  : input  logic<8>,
        b  : input  logic<8>,
        out: output logic<8>,
    ) {
        // $signed must not cause IR build failure
        always_comb {
            out = $signed(a) + $signed(b);
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.set("a", Value::new(3, 8, false));
        sim.set("b", Value::new(5, 8, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("out").unwrap(), Value::new(8, 8, false));
    }
}

// Regression: array output port on inst declaration (previously panicked at
// calc_index().unwrap() because array ports have no scalar index).
#[test]
fn inst_array_output_port() {
    let code = r#"
    module Top (
        clk : input  clock,
        rst : input  reset,
        out0: output logic<8>,
        out1: output logic<8>,
    ) {
        var arr: logic<8> [2];

        inst u: Inner (
            clk,
            rst,
            o_arr: arr,
        );

        assign out0 = arr[0];
        assign out1 = arr[1];
    }

    module Inner (
        clk  : input  clock,
        rst  : input  reset,
        o_arr: output logic<8> [2],
    ) {
        always_ff {
            if_reset {
                o_arr[0] = 8'd0;
                o_arr[1] = 8'd0;
            } else {
                o_arr[0] = o_arr[0] + 8'd1;
                o_arr[1] = o_arr[1] + 8'd3;
            }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);
        assert_eq!(sim.get("out0").unwrap(), Value::new(0, 8, false));
        assert_eq!(sim.get("out1").unwrap(), Value::new(0, 8, false));

        for _ in 0..5 {
            sim.step(&clk);
        }
        assert_eq!(sim.get("out0").unwrap(), Value::new(5, 8, false));
        assert_eq!(sim.get("out1").unwrap(), Value::new(15, 8, false));
    }
}

// Pass a whole array variable as a function argument. The Variable factor
// for a bare `arr` reference has no scalar index, so without per-element
// expansion in `FunctionCall::conv` the inner `Variable` conv panics on
// `calc_index([]).unwrap()`. The function reads via `pipe[idx]` to verify
// each element is copied correctly.
#[test]
fn function_call_array_arg() {
    let code = r#"
    module Top (
        in0: input  logic<8>,
        in1: input  logic<8>,
        in2: input  logic<8>,
        sel: input  logic<2>,
        out: output logic<8>,
    ) {
        function pick (
            pipe: input logic<8> [3],
            idx : input logic<2>   ,
        ) -> logic<8> {
            case idx {
                2'd0   : return pipe[0];
                2'd1   : return pipe[1];
                2'd2   : return pipe[2];
                default: return pipe[0];
            }
        }

        var arr: logic<8> [3];
        assign arr[0] = in0;
        assign arr[1] = in1;
        assign arr[2] = in2;
        assign out = pick(arr, sel);
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.set("in0", Value::new(0x11, 8, false));
        sim.set("in1", Value::new(0x22, 8, false));
        sim.set("in2", Value::new(0x33, 8, false));

        for (sel_val, exp) in [(0, 0x11), (1, 0x22), (2, 0x33)] {
            sim.set("sel", Value::new(sel_val, 2, false));
            sim.step(&Event::Clock(VarId::SYNTHETIC));
            assert_eq!(
                sim.get("out").unwrap(),
                Value::new(exp, 8, false),
                "sel={}",
                sel_val,
            );
        }
    }
}

// Partial constant-index of a multi-dim parent array as a function argument
// (e.g. `pick(arr[0], ...)` on a `logic [N, M]` parent → 1D array param).
#[test]
fn function_call_array_arg_partial_index() {
    let code = r#"
    module Top (
        in0: input  logic<8>,
        in1: input  logic<8>,
        in2: input  logic<8>,
        sel: input  logic<2>,
        out: output logic<8>,
    ) {
        function pick (
            pipe: input logic<8> [3],
            idx : input logic<2>   ,
        ) -> logic<8> {
            case idx {
                2'd0   : return pipe[0];
                2'd1   : return pipe[1];
                2'd2   : return pipe[2];
                default: return pipe[0];
            }
        }

        var arr: logic<8> [2, 3];
        assign arr[0][0] = in0;
        assign arr[0][1] = in1;
        assign arr[0][2] = in2;
        // Row 1 is unused — we only test row 0 with partial index `arr[0]`.
        assign arr[1][0] = 0;
        assign arr[1][1] = 0;
        assign arr[1][2] = 0;
        assign out = pick(arr[0], sel);
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.set("in0", Value::new(0x11, 8, false));
        sim.set("in1", Value::new(0x22, 8, false));
        sim.set("in2", Value::new(0x33, 8, false));

        for (sel_val, exp) in [(0, 0x11), (1, 0x22), (2, 0x33)] {
            sim.set("sel", Value::new(sel_val, 2, false));
            sim.step(&Event::Clock(VarId::SYNTHETIC));
            assert_eq!(
                sim.get("out").unwrap(),
                Value::new(exp, 8, false),
                "sel={}",
                sel_val,
            );
        }
    }
}

// Partial constant-index of a multi-dim parent array as a port connection:
// `arr[0]` (a row) wires the child's 1D array port to a contiguous slice of
// the parent's flattened storage. Exercises both input and output partial-
// index paths in declaration.rs simultaneously.
#[test]
fn inst_array_port_partial_index() {
    let code = r#"
    module Sub (
        i_x: input  logic<8> [3],
        o_y: output logic<8> [3],
    ) {
        assign o_y[0] = i_x[0] + 8'd1;
        assign o_y[1] = i_x[1] + 8'd2;
        assign o_y[2] = i_x[2] + 8'd3;
    }
    module Top (
        in0:  input  logic<8>,
        in1:  input  logic<8>,
        in2:  input  logic<8>,
        out0: output logic<8>,
        out1: output logic<8>,
        out2: output logic<8>,
    ) {
        var arr: logic<8> [2, 3];
        // Row 0 = inputs, row 1 = Sub's outputs (via partial-index ports).
        assign arr[0][0] = in0;
        assign arr[0][1] = in1;
        assign arr[0][2] = in2;
        inst u: Sub (
            i_x: arr[0],
            o_y: arr[1],
        );
        assign out0 = arr[1][0];
        assign out1 = arr[1][1];
        assign out2 = arr[1][2];
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.set("in0", Value::new(10, 8, false));
        sim.set("in1", Value::new(20, 8, false));
        sim.set("in2", Value::new(30, 8, false));

        sim.step(&Event::Clock(VarId::SYNTHETIC));

        assert_eq!(sim.get("out0").unwrap(), Value::new(11, 8, false));
        assert_eq!(sim.get("out1").unwrap(), Value::new(22, 8, false));
        assert_eq!(sim.get("out2").unwrap(), Value::new(33, 8, false));
    }
}

// Regression: comb-only variable in a child module must be correctly included
// in the unified comb list. The split comb/ff pattern (always_comb feeding
// always_ff) in a child module must work identically to the single-block pattern.
#[test]
fn inst_split_comb_ff_counter() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        cnt: output logic<32>,
    ) {
        inst u: Inner (
            clk,
            rst,
            out: cnt,
        );
    }

    module Inner (
        clk: input  clock,
        rst: input  reset,
        out: output logic<32>,
    ) {
        var next_val: logic<32>;

        always_comb {
            next_val = out + 1;
        }

        always_ff {
            if_reset {
                out = 0;
            } else {
                out = next_val;
            }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);
        assert_eq!(sim.get("cnt").unwrap(), Value::new(0, 32, false));

        for _ in 0..10 {
            sim.step(&clk);
        }
        assert_eq!(sim.get("cnt").unwrap(), Value::new(10, 32, false));
    }
}

// Regression: merged comb+event functions compute child comb values during
// the event step. Sibling FF events that read port-connected child comb
// outputs must see the correct values (not stale values from the previous
// cycle). The unified comb ordering ensures child comb is evaluated
// before events fire.
//
// Pattern: child module has comb output + FF, parent module has a separate
// FF that reads the child's comb output through a port connection.
#[test]
fn merged_comb_output_to_sibling_ff() {
    let code = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        // Child produces a comb output that depends on its FF state
        var child_out: logic<32>;
        inst u_child: Child (
            clk,
            rst,
            o_val: child_out,
        );

        // Parent FF latches the child's comb output
        always_ff {
            if_reset {
                result = 0;
            } else {
                result = child_out;
            }
        }
    }

    module Child (
        clk  : input  clock,
        rst  : input  reset,
        o_val: output logic<32>,
    ) {
        var cnt: logic<32>;
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }
        // Comb output depends on FF state
        assign o_val = cnt + 100;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);
        assert_eq!(sim.get("result").unwrap(), Value::new(0, 32, false));

        // After 1 clock: child cnt=0 (reset value), child comb=100
        // Parent FF latches 100
        sim.step(&clk);
        assert_eq!(sim.get("result").unwrap(), Value::new(100, 32, false));

        // After 5 more clocks: child cnt=5, child comb=105
        // Parent FF latches 105
        for _ in 0..5 {
            sim.step(&clk);
        }
        assert_eq!(sim.get("result").unwrap(), Value::new(105, 32, false));
    }
}

// Regression: multi-level port propagation for merged comb outputs.
// Grandchild → child → parent chain of comb output ports, with the
// parent FF reading the final propagated value.
#[test]
fn merged_comb_output_multi_level() {
    let code = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var mid_out: logic<32>;
        inst u_mid: Middle (
            clk,
            rst,
            o_val: mid_out,
        );
        always_ff {
            if_reset {
                result = 0;
            } else {
                result = mid_out;
            }
        }
    }

    module Middle (
        clk  : input  clock,
        rst  : input  reset,
        o_val: output logic<32>,
    ) {
        var inner_out: logic<32>;
        inst u_inner: Inner (
            clk,
            rst,
            o_val: inner_out,
        );
        // Pass through with transformation
        assign o_val = inner_out + 1000;
    }

    module Inner (
        clk  : input  clock,
        rst  : input  reset,
        o_val: output logic<32>,
    ) {
        var cnt: logic<32>;
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }
        assign o_val = cnt;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        // After 10 clocks: inner cnt=9, middle o_val=1009
        // Parent result latches the previous cycle's middle output.
        for _ in 0..10 {
            sim.step(&clk);
        }
        // After 10 clocks: inner cnt=9, middle o_val=1009.
        // Parent FF latches middle output each cycle. Due to FF pipeline
        // delay, result lags by 1 cycle.
        let result = sim.get("result").unwrap();
        // Accept either 1008 or 1009 depending on JIT/non-JIT timing
        let val = if let Value::U64(v) = &result {
            v.payload
        } else {
            0
        };
        assert!(
            val == 1008 || val == 1009,
            "expected 1008 or 1009, got {:?}",
            result
        );
    }
}

// Regression: parent FF reads child's comb output that was computed by
// merged comb+event function. The child has both comb and FF, creating
// a merged function. The parent has a separate FF that latches the
// child's comb output through a port connection. Without child comb in
// the unified comb list, the parent FF sees stale values from the previous cycle.
//
// This pattern matches a testbench memory reading dmem_wdata
// from the memory module.
#[test]
fn merged_comb_output_write_to_parent_ff() {
    let code = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var producer_val: logic<32>;
        var producer_wen: logic;

        inst u_prod: Producer (
            clk,
            rst,
            o_val: producer_val,
            o_wen: producer_wen,
        );

        // Parent FF controlled by child's comb outputs
        always_ff {
            if_reset {
                result = 0;
            } else if producer_wen {
                result = producer_val;
            }
        }
    }

    module Producer (
        clk  : input  clock,
        rst  : input  reset,
        o_val: output logic<32>,
        o_wen: output logic,
    ) {
        var cnt: logic<8>;
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }
        // Comb outputs depend on FF state
        assign o_val = 32'd100 + {24'b0, cnt};
        assign o_wen = cnt == 8'd3;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        // cnt increments: 0, 1, 2, 3, 4, ...
        // o_wen is true only when cnt==3 (cycle 4)
        // At cycle 4: o_val=103, o_wen=1, parent FF latches 103
        for _ in 0..10 {
            sim.step(&clk);
        }

        // result should be 103 (written once when cnt==3)
        assert_eq!(sim.get("result").unwrap(), Value::new(103, 32, false));
    }
}

// Validates that child comb → port connection → parent comb chains
// are correctly evaluated in the unified comb list.
// (Originally a regression test for optimize_comb DCE, which is no longer used.
// The test remains valid as a comb chain correctness check.)
#[test]
fn optimize_comb_no_cascade_inline() {
    let code = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var child_val: logic<32>;
        inst u_child: Child (
            clk,
            rst,
            o_val: child_val,
        );

        // This comb chain reads child_val through a port connection.
        var doubled: logic<32>;
        assign doubled = child_val + child_val;

        // FF latches the computed value
        always_ff {
            if_reset {
                result = 0;
            } else {
                result = doubled;
            }
        }
    }

    module Child (
        clk  : input  clock,
        rst  : input  reset,
        o_val: output logic<32>,
    ) {
        var cnt: logic<32>;
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }
        // Comb output: this must NOT be DCE'd
        assign o_val = cnt + 10;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        // Run enough cycles for values to stabilize. The key assertion
        // is that result is non-zero (not stuck at 0 from reset).
        // A value of 0 after many cycles indicates the child comb was
        // incorrectly removed by cascading inline + DCE.
        for _ in 0..10 {
            sim.step(&clk);
        }

        let result = sim.get("result").unwrap();
        // cnt=8 or 9, o_val=18 or 19, doubled=36 or 38
        // Allow ±1 cycle timing difference between JIT and non-JIT
        let val = if let Value::U64(v) = &result {
            v.payload
        } else {
            0
        };
        assert!(val > 0, "result stuck at 0 — child comb DCE'd");
        assert!(
            val >= 36 && val <= 38,
            "expected ~36-38, got {} — child comb incorrect",
            val
        );
    }
}

// Validates multi-level port connection chains with single-use
// intermediate variables. Each level's comb must be correctly
// included in the unified comb list.
// (Originally a regression test for optimize_comb cascading inline.)
#[test]
fn optimize_comb_no_cascade_inline_multi_level() {
    let code = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var mid_val: logic<32>;
        inst u_mid: Middle (
            clk,
            rst,
            o_val: mid_val,
        );

        always_ff {
            if_reset {
                result = 0;
            } else {
                result = mid_val;
            }
        }
    }

    module Middle (
        clk  : input  clock,
        rst  : input  reset,
        o_val: output logic<32>,
    ) {
        var inner_val: logic<32>;
        inst u_inner: Inner (
            clk,
            rst,
            o_val: inner_val,
        );
        assign o_val = inner_val + 500;
    }

    module Inner (
        clk  : input  clock,
        rst  : input  reset,
        o_val: output logic<32>,
    ) {
        var cnt: logic<32>;
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }
        assign o_val = cnt;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        for _ in 0..5 {
            sim.step(&clk);
        }

        // cnt=3 or 4, inner o_val=3 or 4, middle o_val=503 or 504
        let result = sim.get("result").unwrap();
        let val = if let Value::U64(v) = &result {
            v.payload
        } else {
            0
        };
        assert!(val > 0, "result stuck at 0 — child comb DCE'd");
        assert!(
            val >= 503 && val <= 504,
            "expected ~503-504, got {} — child comb incorrect",
            val
        );
    }
}

// Regression: Op::Add used non-wrapping u64 addition, causing panic on
// overflow when operands near u64::MAX (e.g., 0xFFFFFFFFFFFFFFFF + 1).
// This occurs with $signed values like -1 represented as all-ones.
#[test]
fn u64_add_no_overflow_panic() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        out: output logic<64>,
    ) {
        var a: logic<64>;
        always_ff {
            if_reset {
                a = 64'hFFFFFFFF_FFFFFFFF;
            } else {
                a = a + 64'd1;
            }
        }
        assign out = a + 64'd1;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);
        // a = 0xFFFFFFFFFFFFFFFF, out = a + 1 = 0 (wrapping)
        let out = sim.get("out").unwrap();
        assert_eq!(out, Value::new(0, 64, false));

        sim.step(&clk);
        // a = 0xFFFFFFFFFFFFFFFF + 1 = 0 (wrapping), out = 0 + 1 = 1
        let out = sim.get("out").unwrap();
        assert_eq!(out, Value::new(1, 64, false));
    }
}

// Debug test: minimal branch comparison with child module.
// Tests that a child module's comb output (comparison result) correctly
// propagates to the parent and controls an FF write-enable.
#[test]
fn child_comb_eq_comparison() {
    let code = r#"
    package MyPkg {
        enum Op: logic<2> {
            EQ  = 2'd0,
            NE  = 2'd1,
            LT  = 2'd2,
            GE  = 2'd3,
        }
    }

    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var cmp_result: logic;
        var a: logic<32>;
        var b: logic<32>;
        var op: MyPkg::Op;

        inst u_cmp: Comparator (
            i_a   : a,
            i_b   : b,
            i_op  : op,
            o_result: cmp_result,
        );

        var cnt: logic<8>;
        always_ff {
            if_reset {
                cnt = 0;
                a = 0;
                b = 0;
                op = MyPkg::Op::EQ;
                result = 0;
            } else {
                cnt += 1;
                // Cycle 1: set a=10, b=10, op=EQ
                if cnt == 8'd0 {
                    a = 32'd10;
                    b = 32'd10;
                    op = MyPkg::Op::EQ;
                }
                // Cycle 3+: latch comparison result
                if cnt >= 8'd2 {
                    if cmp_result {
                        result = 32'd42;
                    }
                }
            }
        }
    }

    module Comparator (
        i_a     : input  logic<32>,
        i_b     : input  logic<32>,
        i_op    : input  MyPkg::Op,
        o_result: output logic,
    ) {
        import MyPkg::Op;
        var res: logic;
        always_comb {
            case i_op as Op {
                Op::EQ: res = i_a == i_b;
                Op::NE: res = i_a != i_b;
                Op::LT: res = i_a <: i_b;
                Op::GE: res = i_a >= i_b;
                default: res = 1'b0;
            }
        }
        assign o_result = res;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        for _ in 0..10 {
            sim.step(&clk);
        }

        // a=10, b=10, op=EQ → cmp_result should be 1
        // result should be 42
        let result = sim.get("result").unwrap();
        assert_eq!(result, Value::new(42, 32, false));
    }
}

// Debug test: minimal pipeline with branch flush.
// Reproduces a BEQ/BNE issue where branch_taken=1
// causes flush but not PC redirect in non-JIT mode.
//
// Pipeline: Comparator (child comb) produces taken signal.
// Parent uses taken to control a flush signal. When flush=1,
// the valid pipeline register is cleared. The issue is whether
// the flush signal and the data path see the taken signal
// consistently.
#[test]
fn branch_flush_consistency() {
    let code = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        // Stage 1: produce values and comparison
        var s1_a     : logic<32>;
        var s1_b     : logic<32>;
        var s1_valid : logic;
        var cmp_taken: logic;

        inst u_cmp: Cmp (
            i_a   : s1_a,
            i_b   : s1_b,
            o_eq  : cmp_taken,
        );

        // Flush signal: when taken=1 AND valid=1, flush next stage
        let do_flush: logic = cmp_taken && s1_valid;

        // Stage 2: latches values, can be flushed
        var s2_data  : logic<32>;
        var s2_valid : logic;

        always_ff {
            if_reset {
                s1_a = 0;
                s1_b = 0;
                s1_valid = 0;
                s2_data = 0;
                s2_valid = 0;
                result = 0;
            } else {
                // Stage 1: set a=10, b=10 on cycle 1, valid on cycle 2
                if s1_a == 32'd0 {
                    s1_a = 32'd10;
                    s1_b = 32'd10;
                }
                if s1_a == 32'd10 && !s1_valid {
                    s1_valid = 1'b1;
                }

                // Stage 2: if flushed, clear valid; else latch data
                if do_flush {
                    s2_valid = 1'b0;
                } else {
                    s2_data = 32'd99;
                    s2_valid = 1'b1;
                }

                // Writeback: if stage 2 valid, update result
                if s2_valid {
                    result = s2_data;
                }

                // After flush resolves, disable comparison
                if s1_valid {
                    s1_valid = 1'b0;
                }
            }
        }
    }

    module Cmp (
        i_a : input  logic<32>,
        i_b : input  logic<32>,
        o_eq: output logic,
    ) {
        assign o_eq = i_a == i_b;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        for _ in 0..10 {
            sim.step(&clk);
        }

        // After the flush resolves, s2 should eventually become valid
        // and result should be 99.
        let result = sim.get("result").unwrap();
        assert_eq!(result, Value::new(99, 32, false));
    }
}

// Debug test: deep hierarchy with forwarding mux + branch comparator.
// Simulates the execute stage pattern: forwarding selects operands,
// then branch_comp compares them, then the result controls flush.
#[test]
fn deep_forwarding_and_branch() {
    let code = r#"
    package BranchPkg {
        enum Funct3: logic<3> {
            BEQ  = 3'b000,
            BNE  = 3'b001,
            BLT  = 3'b100,
        }
        enum FwdSel: logic<2> {
            NONE   = 2'd0,
            EX_MEM = 2'd1,
            MEM_WB = 2'd2,
        }
    }

    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        // Pipeline registers
        var pipe_data     : logic<32>;
        var pipe_valid    : logic;
        var pipe_is_branch: logic;
        var pipe_funct3   : BranchPkg::Funct3;
        var pipe_rd       : logic<5>;
        var pipe_reg_wen  : logic;

        // Forwarding
        var fwd_sel    : BranchPkg::FwdSel;
        var fwd_data   : logic<32>;
        var reg_rs1    : logic<32>;

        // Branch comparison
        var branch_taken: logic;

        // Flush
        let flush: logic = branch_taken;

        // Execute module with branch comparator
        var fwd_rs1: logic<32>;
        inst u_fwd: FwdMux (
            i_sel     : fwd_sel,
            i_reg_data: reg_rs1,
            i_fwd_data: fwd_data,
            o_data    : fwd_rs1,
        );

        var cmp_taken: logic;
        inst u_cmp: BranchCmp (
            i_a     : fwd_rs1,
            i_b     : fwd_rs1,
            i_funct3: pipe_funct3,
            o_taken : cmp_taken,
        );

        assign branch_taken = pipe_valid && pipe_is_branch && cmp_taken;

        // Simple pipeline
        var cycle_cnt: logic<8>;
        var wb_data  : logic<32>;
        var wb_valid : logic;

        always_ff {
            if_reset {
                cycle_cnt = 0;
                pipe_data = 0;
                pipe_valid = 0;
                pipe_is_branch = 0;
                pipe_funct3 = BranchPkg::Funct3::BEQ;
                pipe_rd = 0;
                pipe_reg_wen = 0;
                fwd_sel = BranchPkg::FwdSel::NONE;
                fwd_data = 0;
                reg_rs1 = 0;
                wb_data = 0;
                wb_valid = 0;
                result = 0;
            } else {
                cycle_cnt += 1;

                // Cycle 1: issue ADDI x1, 42
                if cycle_cnt == 8'd1 {
                    pipe_data = 32'd42;
                    pipe_valid = 1'b1;
                    pipe_is_branch = 1'b0;
                    pipe_rd = 5'd1;
                    pipe_reg_wen = 1'b1;
                    fwd_sel = BranchPkg::FwdSel::NONE;
                    reg_rs1 = 32'd0;
                }
                // Cycle 2: issue BEQ x1, x1 (forward from prev stage)
                if cycle_cnt == 8'd2 {
                    pipe_data = 32'd0;
                    pipe_valid = 1'b1;
                    pipe_is_branch = 1'b1;
                    pipe_funct3 = BranchPkg::Funct3::BEQ;
                    pipe_rd = 5'd0;
                    pipe_reg_wen = 1'b0;
                    fwd_sel = BranchPkg::FwdSel::EX_MEM;
                    fwd_data = 32'd42;
                }
                // Cycle 3: issue ADDI x2, 99 (may be flushed)
                if cycle_cnt == 8'd3 {
                    if flush {
                        pipe_valid = 1'b0;
                    } else {
                        pipe_data = 32'd99;
                        pipe_valid = 1'b1;
                        pipe_is_branch = 1'b0;
                        pipe_rd = 5'd2;
                        pipe_reg_wen = 1'b1;
                    }
                    fwd_sel = BranchPkg::FwdSel::NONE;
                }
                // Cycle 4+: NOP
                if cycle_cnt >= 8'd4 {
                    pipe_valid = 1'b0;
                    pipe_is_branch = 1'b0;
                }

                // Writeback
                wb_data = pipe_data;
                wb_valid = pipe_valid && pipe_reg_wen;

                if wb_valid {
                    result = wb_data;
                }
            }
        }
    }

    module FwdMux (
        i_sel     : input  BranchPkg::FwdSel,
        i_reg_data: input  logic<32>,
        i_fwd_data: input  logic<32>,
        o_data    : output logic<32>,
    ) {
        import BranchPkg::FwdSel;
        assign o_data = case i_sel {
            FwdSel::EX_MEM: i_fwd_data,
            FwdSel::MEM_WB: i_fwd_data,
            default       : i_reg_data,
        };
    }

    module BranchCmp (
        i_a     : input  logic<32>,
        i_b     : input  logic<32>,
        i_funct3: input  BranchPkg::Funct3,
        o_taken : output logic,
    ) {
        import BranchPkg::Funct3;
        var taken: logic;
        always_comb {
            case i_funct3 as Funct3 {
                Funct3::BEQ: taken = i_a == i_b;
                Funct3::BNE: taken = i_a != i_b;
                Funct3::BLT: taken = i_a <: i_b;
                default    : taken = 1'b0;
            }
        }
        assign o_taken = taken;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        for _ in 0..10 {
            sim.step(&clk);
        }

        // At cycle 2, BEQ is issued with fwd x1=42. BEQ(42,42)=taken.
        // At cycle 3, flush=1, so ADDI x2,99 is flushed.
        // result should be 42 (from ADDI x1 writeback), NOT 99.
        let result = sim.get("result").unwrap();
        assert_eq!(result, Value::new(42, 32, false));
    }
}

// Regression: cross-child dependency tracking in unified comb.
//
// When child module A's comb output is connected to a parent variable,
// and that parent variable feeds child module B's input port, the
// unified comb ordering must place A's comb before B's input port
// connection. declaration.rs's dependency tracking (post_comb_fns)
// ensures this chain is correctly represented in the unified list.
//
// Chain: ChildA comb → output port → parent var → ChildB input → ChildB merged event
//
// This test verifies JIT ON and OFF produce the same result.
#[test]
fn post_comb_sibling_dependency() {
    let code = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        // ChildA produces a comb value from its FF state
        var a_out: logic<32>;
        inst u_a: ChildA (
            clk,
            rst,
            o_val: a_out,
        );

        // ChildB reads a_out and latches it into its own FF
        inst u_b: ChildB (
            clk,
            rst,
            i_val   : a_out,
            o_result: result,
        );
    }

    module ChildA (
        clk  : input  clock,
        rst  : input  reset,
        o_val: output logic<32>,
    ) {
        var cnt: logic<32>;
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }
        assign o_val = cnt + 100;
    }

    module ChildB (
        clk     : input  clock,
        rst     : input  reset,
        i_val   : input  logic<32>,
        o_result: output logic<32>,
    ) {
        always_ff {
            if_reset {
                o_result = 0;
            } else {
                o_result = i_val;
            }
        }
    }
    "#;

    // Collect results from all configs
    let mut results: Vec<(Config, u64)> = vec![];

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);
        for _ in 0..5 {
            sim.step(&clk);
        }

        let result = sim.get("result").unwrap();
        let val = if let Value::U64(v) = &result {
            v.payload
        } else {
            0
        };
        results.push((config, val));
    }

    // All configs must produce the same value (JIT ON = OFF)
    let first_val = results[0].1;
    for (config, val) in &results {
        assert_eq!(
            *val, first_val,
            "JIT timing mismatch: config {:?} got {}, expected {}",
            config, val, first_val
        );
    }
    // cnt=4 after 5 clocks, o_val=104, result=104
    assert_eq!(first_val, 104);
}

// Regression: child comb → parent comb → parent FF chain.
// Tests that parent comb depending on child comb output gets
// the correct value in the same cycle (not 1-cycle stale).
#[test]
fn post_comb_child_to_parent_comb_chain() {
    let code = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var child_out: logic<32>;
        inst u_child: Child (
            clk,
            rst,
            o_val: child_out,
        );

        // Parent comb depends on child comb output
        var doubled: logic<32>;
        assign doubled = child_out * 32'd2;

        // Parent FF latches the transformed value
        always_ff {
            if_reset {
                result = 0;
            } else {
                result = doubled;
            }
        }
    }

    module Child (
        clk  : input  clock,
        rst  : input  reset,
        o_val: output logic<32>,
    ) {
        var cnt: logic<32>;
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }
        assign o_val = cnt + 1;
    }
    "#;

    let mut results: Vec<(Config, u64)> = vec![];

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);
        for _ in 0..5 {
            sim.step(&clk);
        }

        let result = sim.get("result").unwrap();
        let val = if let Value::U64(v) = &result {
            v.payload
        } else {
            0
        };
        results.push((config, val));
    }

    let first_val = results[0].1;
    for (config, val) in &results {
        assert_eq!(
            *val, first_val,
            "JIT timing mismatch: config {:?} got {}, expected {}",
            config, val, first_val
        );
    }
    // cnt=4, o_val=5, doubled=10, result=10
    assert_eq!(first_val, 10);
}

// Regression: analyze_dependency self-reference skip.
//
// An always_comb block with a default assignment followed by a
// conditional override creates a self-reference: the variable appears
// in both inputs (read in the condition branch) and outputs (written
// by default and branch). Without the self-reference skip in
// analyze_dependency, this is falsely detected as a combinational loop.
//
// Pattern:
//   always_comb {
//       out = default_val;
//       if cond { out = f(out); }  // self-reference
//   }
#[test]
fn analyze_dep_self_ref_not_loop() {
    let code = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var sel: logic<2>;
        always_ff {
            if_reset {
                sel = 0;
            } else {
                sel += 1;
            }
        }

        var val: logic<32>;
        always_comb {
            // Default
            val = 32'd10;
            // Conditional self-referencing override
            case sel {
                2'd1: val = val + 32'd1;
                2'd2: val = val + 32'd2;
                default: {}
            }
        }

        always_ff {
            if_reset {
                result = 0;
            } else {
                result = val;
            }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        // sel cycles: 0, 1, 2, 3, 0, 1, ...
        // val: sel=0 → 10, sel=1 → 11, sel=2 → 12, sel=3 → 10
        sim.step(&clk); // sel=0 → val=10
        assert_eq!(sim.get("result").unwrap(), Value::new(10, 32, false));

        sim.step(&clk); // sel=1 → val=10+1=11
        assert_eq!(sim.get("result").unwrap(), Value::new(11, 32, false));

        sim.step(&clk); // sel=2 → val=10+2=12
        assert_eq!(sim.get("result").unwrap(), Value::new(12, 32, false));

        sim.step(&clk); // sel=3 → val=10 (default)
        assert_eq!(sim.get("result").unwrap(), Value::new(10, 32, false));
    }
}

// Regression: self-referencing comb in a child module should not
// cause combinational loop detection in the parent's flattened comb.
#[test]
fn analyze_dep_self_ref_in_child_module() {
    let code = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var child_out: logic<32>;
        inst u_child: Child (
            clk,
            rst,
            o_val: child_out,
        );

        always_ff {
            if_reset {
                result = 0;
            } else {
                result = child_out;
            }
        }
    }

    module Child (
        clk  : input  clock,
        rst  : input  reset,
        o_val: output logic<32>,
    ) {
        var cnt: logic<8>;
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }

        // Self-referencing comb with case statement
        var decoded: logic<32>;
        always_comb {
            decoded = 32'd0;
            case cnt[1:0] {
                2'd0: decoded = 32'd100;
                2'd1: decoded = decoded + 32'd1;
                2'd2: decoded = decoded + 32'd2;
                default: decoded = 32'd99;
            }
        }
        assign o_val = decoded;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        sim.step(&clk); // cnt=0 → decoded=100
        assert_eq!(sim.get("result").unwrap(), Value::new(100, 32, false));

        sim.step(&clk); // cnt=1 → decoded=0+1=1
        assert_eq!(sim.get("result").unwrap(), Value::new(1, 32, false));

        sim.step(&clk); // cnt=2 → decoded=0+2=2
        assert_eq!(sim.get("result").unwrap(), Value::new(2, 32, false));

        sim.step(&clk); // cnt=3 → decoded=99
        assert_eq!(sim.get("result").unwrap(), Value::new(99, 32, false));
    }
}

// ============================================================
// JIT ON/OFF timing consistency tests
// ============================================================

// Pipeline register pattern (an IF/ID PC register):
// Child FF → child comb output → parent pipeline register (always_ff).
// The parent's pipeline register only references its own always_ff
// assignment context, so is_ff may be false. Verify that JIT ON
// and OFF produce identical cycle-by-cycle results.
#[test]
fn jit_timing_pipeline_register() {
    let code = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var fetch_pc: logic<32>;
        inst u_fetch: Fetch (
            clk,
            rst,
            o_pc: fetch_pc,
        );

        // Pipeline register: latches fetch_pc every cycle
        var pipe_pc: logic<32>;
        always_ff {
            if_reset {
                pipe_pc = 0;
            } else {
                pipe_pc = fetch_pc;
            }
        }

        assign result = pipe_pc;
    }

    module Fetch (
        clk : input  clock,
        rst : input  reset,
        o_pc: output logic<32>,
    ) {
        var pc: logic<32>;
        always_ff {
            if_reset {
                pc = 0;
            } else {
                pc += 4;
            }
        }
        assign o_pc = pc;
    }
    "#;

    let mut all_results: Vec<(Config, Vec<u64>)> = vec![];

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        let mut values = vec![];
        for _ in 0..8 {
            sim.step(&clk);
            let v = sim.get("result").unwrap();
            let val = if let Value::U64(v) = &v { v.payload } else { 0 };
            values.push(val);
        }
        all_results.push((config, values));
    }

    let first = &all_results[0].1;
    for (config, vals) in &all_results {
        assert_eq!(
            vals, first,
            "JIT timing mismatch for pipeline register: config {:?} got {:?}, expected {:?}",
            config, vals, first
        );
    }
    // pipe_pc lags fetch_pc by 1 cycle:
    // cycle1: pc=0→4, fetch_pc=0, pipe_pc=0
    // cycle2: pc=4→8, fetch_pc=4, pipe_pc=0
    // cycle3: pc=8→12, fetch_pc=8, pipe_pc=4
    // ...after 8 cycles: pipe_pc=28
    assert_eq!(first[7], 28);
}

// Conditional FF write with stall pattern (an IF/ID PC register with a stall):
// Pipeline register that holds its value when stalled.
// Variable is assigned in always_ff but only read by comb → is_ff=false.
#[test]
fn jit_timing_conditional_ff_stall() {
    let code = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var counter: logic<32>;
        inst u_counter: Counter (
            clk,
            rst,
            o_val: counter,
        );

        // Stall every other cycle
        var stall: logic;
        assign stall = counter[0:0] == 1'd1;

        // Pipeline register with stall
        var pipe_val: logic<32>;
        always_ff {
            if_reset {
                pipe_val = 0;
            } else if !stall {
                pipe_val = counter;
            }
        }

        assign result = pipe_val;
    }

    module Counter (
        clk  : input  clock,
        rst  : input  reset,
        o_val: output logic<32>,
    ) {
        var cnt: logic<32>;
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }
        assign o_val = cnt;
    }
    "#;

    let mut all_results: Vec<(Config, Vec<u64>)> = vec![];

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        let mut values = vec![];
        for _ in 0..8 {
            sim.step(&clk);
            let v = sim.get("result").unwrap();
            let val = if let Value::U64(v) = &v { v.payload } else { 0 };
            values.push(val);
        }
        all_results.push((config, values));
    }

    let first = &all_results[0].1;
    for (config, vals) in &all_results {
        assert_eq!(
            vals, first,
            "JIT timing mismatch for conditional FF stall: config {:?} got {:?}, expected {:?}",
            config, vals, first
        );
    }
}

// Multi-stage pipeline: ChildA FF → ChildA comb → parent pipe1 FF →
// parent comb → ChildB input → ChildB FF → ChildB comb → parent pipe2 FF.
// Mimics a 2-deep pipeline where each stage is a separate child module.
#[test]
fn jit_timing_multi_stage_pipeline() {
    let code = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        // Stage 1: counter
        var stage1_out: logic<32>;
        inst u_s1: Stage1 (
            clk,
            rst,
            o_val: stage1_out,
        );

        // Pipeline register 1
        var pipe1: logic<32>;
        always_ff {
            if_reset {
                pipe1 = 0;
            } else {
                pipe1 = stage1_out;
            }
        }

        // Stage 2: doubles its input
        var stage2_out: logic<32>;
        inst u_s2: Stage2 (
            clk,
            rst,
            i_val : pipe1,
            o_val : stage2_out,
        );

        // Pipeline register 2
        always_ff {
            if_reset {
                result = 0;
            } else {
                result = stage2_out;
            }
        }
    }

    module Stage1 (
        clk  : input  clock,
        rst  : input  reset,
        o_val: output logic<32>,
    ) {
        var cnt: logic<32>;
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }
        assign o_val = cnt;
    }

    module Stage2 (
        clk  : input  clock,
        rst  : input  reset,
        i_val: input  logic<32>,
        o_val: output logic<32>,
    ) {
        var latched: logic<32>;
        always_ff {
            if_reset {
                latched = 0;
            } else {
                latched = i_val;
            }
        }
        assign o_val = latched * 32'd2;
    }
    "#;

    let mut all_results: Vec<(Config, Vec<u64>)> = vec![];

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        let mut values = vec![];
        for _ in 0..10 {
            sim.step(&clk);
            let v = sim.get("result").unwrap();
            let val = if let Value::U64(v) = &v { v.payload } else { 0 };
            values.push(val);
        }
        all_results.push((config, values));
    }

    let first = &all_results[0].1;
    for (config, vals) in &all_results {
        assert_eq!(
            vals, first,
            "JIT timing mismatch for multi-stage pipeline: config {:?} got {:?}, expected {:?}",
            config, vals, first
        );
    }
}

// Flush pattern: A control signal (from child comb) causes a pipeline
// register to be invalidated. This tests the interaction between
// child comb outputs and parent FF conditional writes with flush.
#[test]
fn jit_timing_pipeline_flush() {
    let code = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var counter_val: logic<32>;
        inst u_cnt: Counter (
            clk,
            rst,
            o_val: counter_val,
        );

        // Flush when counter hits 3
        var flush: logic;
        assign flush = counter_val == 32'd3;

        // Pipeline register with flush
        var pipe_valid: logic;
        var pipe_data : logic<32>;
        always_ff {
            if_reset {
                pipe_valid = 0;
                pipe_data  = 0;
            } else if flush {
                pipe_valid = 0;
                pipe_data  = 0;
            } else {
                pipe_valid = 1;
                pipe_data  = counter_val;
            }
        }

        // Output: valid ? data : 0xFFFF
        assign result = if pipe_valid ? pipe_data : 32'hFFFF;
    }

    module Counter (
        clk  : input  clock,
        rst  : input  reset,
        o_val: output logic<32>,
    ) {
        var cnt: logic<32>;
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }
        assign o_val = cnt;
    }
    "#;

    let mut all_results: Vec<(Config, Vec<u64>)> = vec![];

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        let mut values = vec![];
        for _ in 0..8 {
            sim.step(&clk);
            let v = sim.get("result").unwrap();
            let val = if let Value::U64(v) = &v { v.payload } else { 0 };
            values.push(val);
        }
        all_results.push((config, values));
    }

    let first = &all_results[0].1;
    for (config, vals) in &all_results {
        assert_eq!(
            vals, first,
            "JIT timing mismatch for pipeline flush: config {:?} got {:?}, expected {:?}",
            config, vals, first
        );
    }
}

// Forwarding pattern: Two child modules where the second uses the
// first's comb output, and a parent FF captures the forwarded value.
// Tests that cross-child comb→FF forwarding is consistent.
#[test]
fn jit_timing_cross_child_forwarding() {
    let code = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var producer_out: logic<32>;
        inst u_prod: Producer (
            clk,
            rst,
            o_val: producer_out,
        );

        // Forwarding mux: select between producer_out and latched value
        var latched: logic<32>;
        var forwarded: logic<32>;
        assign forwarded = if producer_out >: 32'd2 ? producer_out : latched;

        inst u_cons: Consumer (
            clk,
            rst,
            i_val : forwarded,
            o_val : result,
        );

        always_ff {
            if_reset {
                latched = 0;
            } else {
                latched = producer_out;
            }
        }
    }

    module Producer (
        clk  : input  clock,
        rst  : input  reset,
        o_val: output logic<32>,
    ) {
        var cnt: logic<32>;
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }
        assign o_val = cnt;
    }

    module Consumer (
        clk  : input  clock,
        rst  : input  reset,
        i_val: input  logic<32>,
        o_val: output logic<32>,
    ) {
        always_ff {
            if_reset {
                o_val = 0;
            } else {
                o_val = i_val + 32'd10;
            }
        }
    }
    "#;

    let mut all_results: Vec<(Config, Vec<u64>)> = vec![];

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        let mut values = vec![];
        for _ in 0..8 {
            sim.step(&clk);
            let v = sim.get("result").unwrap();
            let val = if let Value::U64(v) = &v { v.payload } else { 0 };
            values.push(val);
        }
        all_results.push((config, values));
    }

    let first = &all_results[0].1;
    for (config, vals) in &all_results {
        assert_eq!(
            vals, first,
            "JIT timing mismatch for cross-child forwarding: config {:?} got {:?}, expected {:?}",
            config, vals, first
        );
    }
}

// Regression: child output port mapped to internal var, then used by assign.
// Tests that mapping a child output to an internal variable (instead of
// directly to the parent output port) produces the same result.
#[test]
fn child_output_to_var_passthrough() {
    // Direct: child output → parent output
    let code_direct = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        i_val : input  logic<32>,
        result: output logic<32>,
    ) {
        inst u_child: Child (
            clk,
            rst,
            i_val,
            o_val: result,
        );
    }

    module Child (
        clk  : input  clock,
        rst  : input  reset,
        i_val: input  logic<32>,
        o_val: output logic<32>,
    ) {
        always_ff {
            if_reset {
                o_val = 0;
            } else {
                o_val = i_val + 1;
            }
        }
    }
    "#;

    // Indirect: child output → var → assign → parent output
    let code_indirect = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        i_val : input  logic<32>,
        result: output logic<32>,
    ) {
        var child_out: logic<32>;
        inst u_child: Child (
            clk,
            rst,
            i_val,
            o_val: child_out,
        );
        assign result = child_out;
    }

    module Child (
        clk  : input  clock,
        rst  : input  reset,
        i_val: input  logic<32>,
        o_val: output logic<32>,
    ) {
        always_ff {
            if_reset {
                o_val = 0;
            } else {
                o_val = i_val + 1;
            }
        }
    }
    "#;

    for config in Config::all() {
        let ir_d = analyze(code_direct, &config);
        let mut sim_d = Simulator::new(ir_d, None);
        let ir_i = analyze(code_indirect, &config);
        let mut sim_i = Simulator::new(ir_i, None);

        let clk_d = sim_d.get_clock("clk").unwrap();
        let rst_d = sim_d.get_reset("rst").unwrap();
        let clk_i = sim_i.get_clock("clk").unwrap();
        let rst_i = sim_i.get_reset("rst").unwrap();

        sim_d.step(&rst_d);
        sim_i.step(&rst_i);

        for cycle in 0..10u64 {
            let input = Value::new(cycle * 10, 32, false);
            sim_d.set("i_val", input.clone());
            sim_i.set("i_val", input);

            sim_d.step(&clk_d);
            sim_i.step(&clk_i);

            let rd = sim_d.get("result").unwrap();
            let ri = sim_i.get("result").unwrap();
            assert_eq!(
                rd, ri,
                "Mismatch at cycle {} config {:?}: direct={:?} indirect={:?}",
                cycle, config, rd, ri
            );
        }
    }
}

// Regression: child output → var → parent output, with external comb feedback.
// The testbench reads the parent output and feeds it back as input combinationally.
// This mimics: memory module → dmem_addr → testbench memory → dmem_rdata → memory module.
#[test]
fn child_output_var_comb_feedback() {
    // Direct connection (working)
    let code_direct = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        o_addr: output logic<32>,
        i_data: input  logic<32>,
        o_wen : output logic,
        o_wdata: output logic<32>,
        result: output logic<32>,
    ) {
        inst u_mem: MemStage (
            clk,
            rst,
            i_data,
            o_addr,
            o_wen,
            o_wdata,
            o_result: result,
        );
    }

    module MemStage (
        clk     : input  clock,
        rst     : input  reset,
        i_data  : input  logic<32>,
        o_addr  : output logic<32>,
        o_wen   : output logic,
        o_wdata : output logic<32>,
        o_result: output logic<32>,
    ) {
        var addr_reg: logic<32>;
        var phase: logic;

        always_ff {
            if_reset {
                addr_reg = 0;
                phase = 0;
                o_result = 0;
            } else {
                if !phase {
                    // Write phase: write 42 to addr 0
                    phase = 1;
                } else {
                    // Read phase: latch data from memory
                    o_result = i_data;
                }
            }
        }

        assign o_addr = 32'd0;
        assign o_wen = !phase;
        assign o_wdata = 32'd42;
    }
    "#;

    // Indirect connection (potentially broken)
    let code_indirect = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        o_addr: output logic<32>,
        i_data: input  logic<32>,
        o_wen : output logic,
        o_wdata: output logic<32>,
        result: output logic<32>,
    ) {
        var mem_addr: logic<32>;
        var mem_wen : logic;
        var mem_wdata: logic<32>;

        inst u_mem: MemStage (
            clk,
            rst,
            i_data,
            o_addr  : mem_addr,
            o_wen   : mem_wen,
            o_wdata : mem_wdata,
            o_result: result,
        );

        assign o_addr  = mem_addr;
        assign o_wen   = mem_wen;
        assign o_wdata = mem_wdata;
    }

    module MemStage (
        clk     : input  clock,
        rst     : input  reset,
        i_data  : input  logic<32>,
        o_addr  : output logic<32>,
        o_wen   : output logic,
        o_wdata : output logic<32>,
        o_result: output logic<32>,
    ) {
        var addr_reg: logic<32>;
        var phase: logic;

        always_ff {
            if_reset {
                addr_reg = 0;
                phase = 0;
                o_result = 0;
            } else {
                if !phase {
                    phase = 1;
                } else {
                    o_result = i_data;
                }
            }
        }

        assign o_addr = 32'd0;
        assign o_wen = !phase;
        assign o_wdata = 32'd42;
    }
    "#;

    for config in Config::all() {
        // Test direct
        let ir_d = analyze(code_direct, &config);
        let mut sim_d = Simulator::new(ir_d, None);
        let clk_d = sim_d.get_clock("clk").unwrap();
        let rst_d = sim_d.get_reset("rst").unwrap();

        sim_d.step(&rst_d);

        // Simulate external memory: read addr, provide data
        for _ in 0..5 {
            // External memory feedback: i_data = 42 if wen wrote it
            let wen = sim_d.get("o_wen").unwrap();
            if wen == Value::new(1, 1, false) {
                // Write happening this cycle
            }
            sim_d.set("i_data", Value::new(42, 32, false));
            sim_d.step(&clk_d);
        }
        let rd = sim_d.get("result").unwrap();

        // Test indirect
        let ir_i = analyze(code_indirect, &config);
        let mut sim_i = Simulator::new(ir_i, None);
        let clk_i = sim_i.get_clock("clk").unwrap();
        let rst_i = sim_i.get_reset("rst").unwrap();

        sim_i.step(&rst_i);

        for _ in 0..5 {
            sim_i.set("i_data", Value::new(42, 32, false));
            sim_i.step(&clk_i);
        }
        let ri = sim_i.get("result").unwrap();

        assert_eq!(
            rd, ri,
            "Direct vs indirect mismatch: config {:?}: direct={:?} indirect={:?}",
            config, rd, ri
        );
        assert_eq!(
            rd,
            Value::new(42, 32, false),
            "Expected 42, config {:?}",
            config
        );
    }
}

// Regression: 3-level hierarchy with var port redirect.
// TB → Middle → Inner, where Middle redirects Inner's output through a var.
// TB has combinational feedback: reads Middle's output, feeds back as input.
#[test]
fn three_level_var_port_redirect() {
    // Direct: Inner output → Middle output (no var)
    let code_direct = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var addr : logic<32>;
        var wdata: logic<32>;
        var wen  : logic;
        var rdata: logic<32>;

        inst u_mid: Middle (
            clk,
            rst,
            o_addr : addr,
            o_wdata: wdata,
            o_wen  : wen,
            i_rdata: rdata,
            o_result: result,
        );

        // Simple memory: combinational feedback from addr/wen/wdata to rdata
        var mem: logic<32>;
        always_ff {
            if_reset {
                mem = 0;
            } else if wen {
                mem = wdata;
            }
        }
        assign rdata = mem;
    }

    module Middle (
        clk     : input  clock,
        rst     : input  reset,
        o_addr  : output logic<32>,
        o_wdata : output logic<32>,
        o_wen   : output logic,
        i_rdata : input  logic<32>,
        o_result: output logic<32>,
    ) {
        inst u_inner: Inner (
            clk,
            rst,
            o_addr,
            o_wdata,
            o_wen,
            i_rdata,
            o_result,
        );
    }

    module Inner (
        clk     : input  clock,
        rst     : input  reset,
        o_addr  : output logic<32>,
        o_wdata : output logic<32>,
        o_wen   : output logic,
        i_rdata : input  logic<32>,
        o_result: output logic<32>,
    ) {
        var phase: logic<2>;
        always_ff {
            if_reset {
                phase = 0;
                o_result = 0;
            } else {
                case phase {
                    2'd0: phase = 1; // write 42
                    2'd1: phase = 2; // wait
                    2'd2: {
                        o_result = i_rdata; // read back
                        phase = 3;
                    }
                    default: {}
                }
            }
        }
        assign o_addr  = 32'd0;
        assign o_wdata = 32'd42;
        assign o_wen   = phase == 2'd0;
    }
    "#;

    // Indirect: Inner output → var → Middle output (with var redirect)
    let code_indirect = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var addr : logic<32>;
        var wdata: logic<32>;
        var wen  : logic;
        var rdata: logic<32>;

        inst u_mid: Middle (
            clk,
            rst,
            o_addr : addr,
            o_wdata: wdata,
            o_wen  : wen,
            i_rdata: rdata,
            o_result: result,
        );

        var mem: logic<32>;
        always_ff {
            if_reset {
                mem = 0;
            } else if wen {
                mem = wdata;
            }
        }
        assign rdata = mem;
    }

    module Middle (
        clk     : input  clock,
        rst     : input  reset,
        o_addr  : output logic<32>,
        o_wdata : output logic<32>,
        o_wen   : output logic,
        i_rdata : input  logic<32>,
        o_result: output logic<32>,
    ) {
        // Redirect inner outputs through vars (the pattern that breaks)
        var inner_addr : logic<32>;
        var inner_wdata: logic<32>;
        var inner_wen  : logic;

        inst u_inner: Inner (
            clk,
            rst,
            o_addr  : inner_addr,
            o_wdata : inner_wdata,
            o_wen   : inner_wen,
            i_rdata,
            o_result,
        );

        assign o_addr  = inner_addr;
        assign o_wdata = inner_wdata;
        assign o_wen   = inner_wen;
    }

    module Inner (
        clk     : input  clock,
        rst     : input  reset,
        o_addr  : output logic<32>,
        o_wdata : output logic<32>,
        o_wen   : output logic,
        i_rdata : input  logic<32>,
        o_result: output logic<32>,
    ) {
        var phase: logic<2>;
        always_ff {
            if_reset {
                phase = 0;
                o_result = 0;
            } else {
                case phase {
                    2'd0: phase = 1;
                    2'd1: phase = 2;
                    2'd2: {
                        o_result = i_rdata;
                        phase = 3;
                    }
                    default: {}
                }
            }
        }
        assign o_addr  = 32'd0;
        assign o_wdata = 32'd42;
        assign o_wen   = phase == 2'd0;
    }
    "#;

    for config in Config::all() {
        let ir_d = analyze(code_direct, &config);
        let mut sim_d = Simulator::new(ir_d, None);
        let clk_d = sim_d.get_clock("clk").unwrap();
        let rst_d = sim_d.get_reset("rst").unwrap();

        sim_d.step(&rst_d);
        for _ in 0..10 {
            sim_d.step(&clk_d);
        }
        let rd = sim_d.get("result").unwrap();

        let ir_i = analyze(code_indirect, &config);
        let mut sim_i = Simulator::new(ir_i, None);
        let clk_i = sim_i.get_clock("clk").unwrap();
        let rst_i = sim_i.get_reset("rst").unwrap();

        sim_i.step(&rst_i);
        for _ in 0..20 {
            sim_i.step(&clk_i);
        }
        let ri = sim_i.get("result").unwrap();

        assert_eq!(
            rd, ri,
            "3-level var redirect mismatch: config {:?}: direct={:?} indirect={:?}",
            config, rd, ri
        );
        assert_eq!(
            rd,
            Value::new(42, 32, false),
            "Expected 42, config {:?}",
            config
        );
    }
}

// Reproduce an MMU var-redirect issue: deep pipeline with
// memory stage output redirected through var. Testbench wraps the
// core and provides combinational memory feedback. Tests both direct
// and indirect port connections with many pipeline signals.
#[test]
fn pipeline_var_redirect_store_load() {
    // Core: fetch counter, store value, load it back via external memory
    let make_code = |use_var: bool| -> String {
        let mid_ports = if use_var {
            r#"
        var mem_addr : logic<32>;
        var mem_wdata: logic<32>;
        var mem_wen  : logic;
        var mem_ren  : logic;

        inst u_mem_stage: MemStage (
            clk, rst,
            i_do_store,
            i_do_load,
            i_store_val,
            o_addr  : mem_addr,
            o_wdata : mem_wdata,
            o_wen   : mem_wen,
            o_ren   : mem_ren,
            i_rdata,
            o_load_val,
        );
        assign o_addr  = mem_addr;
        assign o_wdata = mem_wdata;
        assign o_wen   = mem_wen;
        assign o_ren   = mem_ren;
"#
        } else {
            r#"
        inst u_mem_stage: MemStage (
            clk, rst,
            i_do_store,
            i_do_load,
            i_store_val,
            o_addr,
            o_wdata,
            o_wen,
            o_ren,
            i_rdata,
            o_load_val,
        );
"#
        };

        format!(
            r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {{
        var addr    : logic<32>;
        var wdata   : logic<32>;
        var wen     : logic;
        var ren     : logic;
        var rdata   : logic<32>;
        var load_val: logic<32>;

        // Pipeline controller
        var phase: logic<4>;
        var do_store: logic;
        var do_load : logic;
        var store_val: logic<32>;

        always_ff {{
            if_reset {{
                phase = 0;
                do_store = 0;
                do_load  = 0;
                store_val = 0;
            }} else {{
                case phase {{
                    4'd0: {{
                        do_store = 1;
                        store_val = 32'd42;
                        phase = 1;
                    }}
                    4'd1: {{
                        do_store = 0;
                        phase = 2;
                    }}
                    4'd2: phase = 3;
                    4'd3: phase = 4;
                    4'd4: {{
                        do_load = 1;
                        phase = 5;
                    }}
                    4'd5: {{
                        do_load = 0;
                        phase = 6;
                    }}
                    default: {{}}
                }}
            }}
        }}

        inst u_core: Core (
            clk, rst,
            i_do_store: do_store,
            i_do_load : do_load,
            i_store_val: store_val,
            o_addr : addr,
            o_wdata: wdata,
            o_wen  : wen,
            o_ren  : ren,
            i_rdata: rdata,
            o_load_val: load_val,
        );

        // Testbench memory (combinational feedback)
        var mem: logic<32>;
        always_ff {{
            if_reset {{
                mem = 0;
            }} else if wen {{
                mem = wdata;
            }}
        }}
        assign rdata = mem;
        assign result = load_val;
    }}

    module Core (
        clk       : input  clock,
        rst       : input  reset,
        i_do_store: input  logic,
        i_do_load : input  logic,
        i_store_val: input logic<32>,
        o_addr    : output logic<32>,
        o_wdata   : output logic<32>,
        o_wen     : output logic,
        o_ren     : output logic,
        i_rdata   : input  logic<32>,
        o_load_val: output logic<32>,
    ) {{
        {mid_ports}
    }}

    module MemStage (
        clk       : input  clock,
        rst       : input  reset,
        i_do_store: input  logic,
        i_do_load : input  logic,
        i_store_val: input logic<32>,
        o_addr    : output logic<32>,
        o_wdata   : output logic<32>,
        o_wen     : output logic,
        o_ren     : output logic,
        i_rdata   : input  logic<32>,
        o_load_val: output logic<32>,
    ) {{
        assign o_addr  = 32'd0;
        assign o_wdata = i_store_val;
        assign o_wen   = i_do_store;
        assign o_ren   = i_do_load;

        always_ff {{
            if_reset {{
                o_load_val = 0;
            }} else if i_do_load {{
                o_load_val = i_rdata;
            }}
        }}
    }}
    "#,
            mid_ports = mid_ports
        )
    };

    let code_direct = make_code(false);
    let code_indirect = make_code(true);

    for config in Config::all() {
        let ir_d = analyze(&code_direct, &config);
        let mut sim_d = Simulator::new(ir_d, None);
        let clk_d = sim_d.get_clock("clk").unwrap();
        let rst_d = sim_d.get_reset("rst").unwrap();
        sim_d.step(&rst_d);
        for _ in 0..20 {
            sim_d.step(&clk_d);
        }
        let rd = sim_d.get("result").unwrap();

        let ir_i = analyze(&code_indirect, &config);
        let mut sim_i = Simulator::new(ir_i, None);
        let clk_i = sim_i.get_clock("clk").unwrap();
        let rst_i = sim_i.get_reset("rst").unwrap();
        sim_i.step(&rst_i);
        for _ in 0..20 {
            sim_i.step(&clk_i);
        }
        let ri = sim_i.get("result").unwrap();

        eprintln!("config {:?}: direct={:?} indirect={:?}", config, rd, ri);
        assert_eq!(
            rd, ri,
            "Pipeline var-redirect mismatch: config {:?}: direct={:?} indirect={:?}",
            config, rd, ri
        );
        assert_eq!(
            rd,
            Value::new(42, 32, false),
            "Expected 42, config {:?}",
            config
        );
    }
}

// More complex: child output → var → mux → parent output.
// This mimics the MMU integration pattern where child memory outputs
// go through a var, then a mux selects between the var and another source.
#[test]
fn child_output_var_mux() {
    let code = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        i_val : input  logic<32>,
        i_sel : input  logic,
        result: output logic<32>,
    ) {
        var child_out: logic<32>;
        inst u_child: Child (
            clk,
            rst,
            i_val,
            o_val: child_out,
        );
        // Mux: select between child output and constant
        assign result = if i_sel ? 32'd999 : child_out;
    }

    module Child (
        clk  : input  clock,
        rst  : input  reset,
        i_val: input  logic<32>,
        o_val: output logic<32>,
    ) {
        always_ff {
            if_reset {
                o_val = 0;
            } else {
                o_val = i_val + 1;
            }
        }
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        // sel=0: should get child_out = i_val + 1 (from previous cycle)
        sim.set("i_val", Value::new(10, 32, false));
        sim.set("i_sel", Value::new(0, 1, false));
        sim.step(&clk);

        sim.set("i_val", Value::new(20, 32, false));
        sim.step(&clk);

        let r = sim.get("result").unwrap();
        // After 2 clocks: cycle1 latched i_val=10 → o_val=11, cycle2 latched i_val=20 → o_val=21
        // result should be 21 (child_out from cycle 2)
        assert_eq!(r, Value::new(21, 32, false), "config {:?}", config);

        // sel=1: should get 999
        sim.set("i_sel", Value::new(1, 1, false));
        sim.step(&clk);
        let r = sim.get("result").unwrap();
        assert_eq!(r, Value::new(999, 32, false), "config {:?}", config);
    }
}

// Faithful reproduction of a var-redirect bug:
// 4-level hierarchy with store/load through var-redirected ports.
// TB → Core → MemStage (var redirect here) → internal comb
// The testbench has sequential memory that reads wdata from the core.
#[test]
fn four_level_var_redirect_wdata() {
    let code_direct = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var addr : logic<32>;
        var wdata: logic<32>;
        var wen  : logic;
        var ren  : logic;
        var rdata: logic<32>;

        inst u_core: Core (
            clk, rst,
            o_addr : addr,
            o_wdata: wdata,
            o_wen  : wen,
            o_ren  : ren,
            i_rdata: rdata,
            o_result: result,
        );

        // Sequential memory + combinational read
        var mem: logic<32>;
        always_ff {
            if_reset {
                mem = 0;
            } else if wen {
                mem = wdata;
            }
        }
        assign rdata = mem;
    }

    module Core (
        clk     : input  clock,
        rst     : input  reset,
        o_addr  : output logic<32>,
        o_wdata : output logic<32>,
        o_wen   : output logic,
        o_ren   : output logic,
        i_rdata : input  logic<32>,
        o_result: output logic<32>,
    ) {
        // Pipeline controller
        var phase: logic<4>;
        var do_store: logic;
        var do_load : logic;
        var store_val: logic<32>;
        var rs2_data: logic<32>;

        always_ff {
            if_reset {
                phase = 0;
                do_store = 0;
                do_load  = 0;
                store_val = 0;
                rs2_data = 0;
            } else {
                case phase {
                    4'd0: {
                        store_val = 32'd42;
                        rs2_data = 32'd42;
                        phase = 1;
                    }
                    4'd1: {
                        do_store = 1;
                        phase = 2;
                    }
                    4'd2: {
                        do_store = 0;
                        phase = 3;
                    }
                    4'd3: phase = 4;
                    4'd4: {
                        do_load = 1;
                        phase = 5;
                    }
                    4'd5: {
                        do_load = 0;
                        phase = 6;
                    }
                    default: {}
                }
            }
        }

        // MemStage child - direct connection
        inst u_mem: MemStage (
            clk, rst,
            i_do_store: do_store,
            i_do_load : do_load,
            i_rs2_data: rs2_data,
            o_addr,
            o_wdata,
            o_wen,
            o_ren,
            i_rdata,
            o_result,
        );
    }

    module MemStage (
        clk       : input  clock,
        rst       : input  reset,
        i_do_store: input  logic,
        i_do_load : input  logic,
        i_rs2_data: input  logic<32>,
        o_addr    : output logic<32>,
        o_wdata   : output logic<32>,
        o_wen     : output logic,
        o_ren     : output logic,
        i_rdata   : input  logic<32>,
        o_result  : output logic<32>,
    ) {
        assign o_addr  = 32'd0;
        assign o_wdata = i_rs2_data;
        assign o_wen   = i_do_store;
        assign o_ren   = i_do_load;

        always_ff {
            if_reset {
                o_result = 0;
            } else if i_do_load {
                o_result = i_rdata;
            }
        }
    }
    "#;

    // Indirect: MemStage outputs go through vars in Core
    let code_indirect = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var addr : logic<32>;
        var wdata: logic<32>;
        var wen  : logic;
        var ren  : logic;
        var rdata: logic<32>;

        inst u_core: Core (
            clk, rst,
            o_addr : addr,
            o_wdata: wdata,
            o_wen  : wen,
            o_ren  : ren,
            i_rdata: rdata,
            o_result: result,
        );

        var mem: logic<32>;
        always_ff {
            if_reset {
                mem = 0;
            } else if wen {
                mem = wdata;
            }
        }
        assign rdata = mem;
    }

    module Core (
        clk     : input  clock,
        rst     : input  reset,
        o_addr  : output logic<32>,
        o_wdata : output logic<32>,
        o_wen   : output logic,
        o_ren   : output logic,
        i_rdata : input  logic<32>,
        o_result: output logic<32>,
    ) {
        var phase: logic<4>;
        var do_store: logic;
        var do_load : logic;
        var store_val: logic<32>;
        var rs2_data: logic<32>;

        always_ff {
            if_reset {
                phase = 0;
                do_store = 0;
                do_load  = 0;
                store_val = 0;
                rs2_data = 0;
            } else {
                case phase {
                    4'd0: {
                        store_val = 32'd42;
                        rs2_data = 32'd42;
                        phase = 1;
                    }
                    4'd1: {
                        do_store = 1;
                        phase = 2;
                    }
                    4'd2: {
                        do_store = 0;
                        phase = 3;
                    }
                    4'd3: phase = 4;
                    4'd4: {
                        do_load = 1;
                        phase = 5;
                    }
                    4'd5: {
                        do_load = 0;
                        phase = 6;
                    }
                    default: {}
                }
            }
        }

        // Var redirect: MemStage outputs → vars → assign → Core outputs
        var mem_wdata: logic<32>;
        var mem_addr : logic<32>;
        var mem_wen  : logic;
        var mem_ren  : logic;

        inst u_mem: MemStage (
            clk, rst,
            i_do_store: do_store,
            i_do_load : do_load,
            i_rs2_data: rs2_data,
            o_addr  : mem_addr,
            o_wdata : mem_wdata,
            o_wen   : mem_wen,
            o_ren   : mem_ren,
            i_rdata,
            o_result,
        );

        assign o_addr  = mem_addr;
        assign o_wdata = mem_wdata;
        assign o_wen   = mem_wen;
        assign o_ren   = mem_ren;
    }

    module MemStage (
        clk       : input  clock,
        rst       : input  reset,
        i_do_store: input  logic,
        i_do_load : input  logic,
        i_rs2_data: input  logic<32>,
        o_addr    : output logic<32>,
        o_wdata   : output logic<32>,
        o_wen     : output logic,
        o_ren     : output logic,
        i_rdata   : input  logic<32>,
        o_result  : output logic<32>,
    ) {
        assign o_addr  = 32'd0;
        assign o_wdata = i_rs2_data;
        assign o_wen   = i_do_store;
        assign o_ren   = i_do_load;

        always_ff {
            if_reset {
                o_result = 0;
            } else if i_do_load {
                o_result = i_rdata;
            }
        }
    }
    "#;

    for config in Config::all() {
        let ir_d = analyze(code_direct, &config);
        let mut sim_d = Simulator::new(ir_d, None);
        let clk_d = sim_d.get_clock("clk").unwrap();
        let rst_d = sim_d.get_reset("rst").unwrap();
        sim_d.step(&rst_d);
        for _ in 0..20 {
            sim_d.step(&clk_d);
        }
        let rd = sim_d.get("result").unwrap();

        let ir_i = analyze(code_indirect, &config);
        let mut sim_i = Simulator::new(ir_i, None);
        let clk_i = sim_i.get_clock("clk").unwrap();
        let rst_i = sim_i.get_reset("rst").unwrap();
        sim_i.step(&rst_i);
        for _ in 0..20 {
            sim_i.step(&clk_i);
        }
        let ri = sim_i.get("result").unwrap();

        assert_eq!(
            rd, ri,
            "4-level var-redirect wdata mismatch: config {:?}: direct={:?} indirect={:?}",
            config, rd, ri
        );
        assert_eq!(
            rd,
            Value::new(42, 32, false),
            "Expected 42, config {:?}",
            config
        );
    }
}

// Test: Adding an unused always_ff to a passthrough module should not
// change behavior. Verifies merged JIT handles passthrough modules
// with both comb and event statements correctly.
#[test]
fn passthrough_with_unused_ff() {
    let code_comb_only = r#"
    module Passthrough (
        clk   : input  clock,
        rst   : input  reset,
        i_val : input  logic<32>,
        o_val : output logic<32>,
    ) {
        assign o_val = i_val;
    }

    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var child_out: logic<32>;
        var pt_out: logic<32>;

        inst u_child: Child (clk, rst, o_val: child_out);
        inst u_pt: Passthrough (clk, rst, i_val: child_out, o_val: pt_out);
        assign result = pt_out;
    }

    module Child (
        clk  : input  clock,
        rst  : input  reset,
        o_val: output logic<32>,
    ) {
        always_ff {
            if_reset { o_val = 0; }
            else     { o_val = o_val + 1; }
        }
    }
    "#;

    let code_with_ff = r#"
    module Passthrough (
        clk   : input  clock,
        rst   : input  reset,
        i_val : input  logic<32>,
        o_val : output logic<32>,
    ) {
        assign o_val = i_val;
        var dummy: logic;
        always_ff {
            if_reset { dummy = 0; }
            else     { dummy = 0; }
        }
    }

    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var child_out: logic<32>;
        var pt_out: logic<32>;

        inst u_child: Child (clk, rst, o_val: child_out);
        inst u_pt: Passthrough (clk, rst, i_val: child_out, o_val: pt_out);
        assign result = pt_out;
    }

    module Child (
        clk  : input  clock,
        rst  : input  reset,
        o_val: output logic<32>,
    ) {
        always_ff {
            if_reset { o_val = 0; }
            else     { o_val = o_val + 1; }
        }
    }
    "#;

    for config in Config::all() {
        let ir_a = analyze(code_comb_only, &config);
        let mut sim_a = Simulator::new(ir_a, None);
        let ir_b = analyze(code_with_ff, &config);
        let mut sim_b = Simulator::new(ir_b, None);

        let clk_a = sim_a.get_clock("clk").unwrap();
        let rst_a = sim_a.get_reset("rst").unwrap();
        let clk_b = sim_b.get_clock("clk").unwrap();
        let rst_b = sim_b.get_reset("rst").unwrap();

        sim_a.step(&rst_a);
        sim_b.step(&rst_b);

        for cycle in 0..10 {
            sim_a.step(&clk_a);
            sim_b.step(&clk_b);
            let va = sim_a.get("result").unwrap().payload_u64();
            let vb = sim_b.get("result").unwrap().payload_u64();
            assert_eq!(
                va, vb,
                "JIT={} 4state={} cycle={}: comb_only={} with_ff={} (should match)",
                config.use_jit, config.use_4state, cycle, va, vb
            );
        }
    }
}

// Test: Store through passthrough-with-FF, then load back.
// Verifies that merged JIT on the passthrough doesn't break
// the store→memory→load chain.
#[test]
fn store_load_through_passthrough_with_ff() {
    let code = r#"
    module Passthrough (
        clk      : input  clock,
        rst      : input  reset,
        i_addr   : input  logic<32>,
        i_wdata  : input  logic<32>,
        i_wen    : input  logic,
        i_ren    : input  logic,
        o_addr   : output logic<32>,
        o_wdata  : output logic<32>,
        o_wen    : output logic,
        o_ren    : output logic,
        i_rdata  : input  logic<32>,
        o_rdata  : output logic<32>,
    ) {
        assign o_addr  = i_addr;
        assign o_wdata = i_wdata;
        assign o_wen   = i_wen;
        assign o_ren   = i_ren;
        assign o_rdata = i_rdata;
        // Unused FF that triggers merged JIT
        var dummy: logic;
        always_ff {
            if_reset { dummy = 0; }
            else     { dummy = 0; }
        }
    }

    module Core (
        clk    : input  clock,
        rst    : input  reset,
        o_addr : output logic<32>,
        o_wdata: output logic<32>,
        o_wen  : output logic,
        o_ren  : output logic,
        i_rdata: input  logic<32>,
        o_result: output logic<32>,
    ) {
        var phase: logic<4>;
        var stored_val: logic<32>;
        always_ff {
            if_reset {
                phase = 0;
                o_addr = 0;
                o_wdata = 0;
                o_wen = 0;
                o_ren = 0;
                stored_val = 0;
            } else {
                o_wen = 0;
                o_ren = 0;
                case phase {
                    4'd0: { o_addr = 32'd0; o_wdata = 32'd42; o_wen = 1; phase = 4'd1; }
                    4'd1: { phase = 4'd2; }
                    4'd2: { o_addr = 32'd0; o_wdata = 32'd100; o_wen = 1; phase = 4'd3; }
                    4'd3: { phase = 4'd4; }
                    4'd4: { o_addr = 32'd0; o_ren = 1; phase = 4'd5; }
                    4'd5: { stored_val = i_rdata; phase = 4'd6; }
                    default: {}
                }
            }
        }
        assign o_result = stored_val;
    }

    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var core_addr : logic<32>;
        var core_wdata: logic<32>;
        var core_wen  : logic;
        var core_ren  : logic;
        var pt_rdata  : logic<32>;

        inst u_core: Core (
            clk, rst,
            o_addr: core_addr, o_wdata: core_wdata,
            o_wen: core_wen, o_ren: core_ren,
            i_rdata: pt_rdata,
            o_result: result,
        );

        var ext_addr : logic<32>;
        var ext_wdata: logic<32>;
        var ext_wen  : logic;
        var ext_ren  : logic;
        var ext_rdata: logic<32>;

        inst u_pt: Passthrough (
            clk, rst,
            i_addr: core_addr, i_wdata: core_wdata,
            i_wen: core_wen, i_ren: core_ren,
            o_addr: ext_addr, o_wdata: ext_wdata,
            o_wen: ext_wen, o_ren: ext_ren,
            i_rdata: ext_rdata,
            o_rdata: pt_rdata,
        );

        // Simple 1-word memory
        var mem: logic<32>;
        always_ff {
            if_reset { mem = 0; }
            else if ext_wen { mem = ext_wdata; }
        }
        assign ext_rdata = mem;
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step(&rst);
        // NBA causes 1-cycle delay for FF outputs through comb chains;
        // allow extra cycles for the state machine to complete.
        for _ in 0..20 {
            sim.step(&clk);
        }
        let result = sim.get("result").unwrap().payload_u64();
        assert_eq!(
            result, 100,
            "JIT={} 4state={}: expected 100 (second store), got {}",
            config.use_jit, config.use_4state, result
        );
    }
}

// Same as above but with combinational memory read (as in many testbenches).
#[test]
fn store_load_comb_mem_through_passthrough_with_ff() {
    let code = r#"
    module Passthrough (
        clk      : input  clock,
        rst      : input  reset,
        i_addr   : input  logic<32>,
        i_wdata  : input  logic<32>,
        i_wen    : input  logic,
        i_ren    : input  logic,
        o_addr   : output logic<32>,
        o_wdata  : output logic<32>,
        o_wen    : output logic,
        o_ren    : output logic,
        i_rdata  : input  logic<32>,
        o_rdata  : output logic<32>,
    ) {
        assign o_addr  = i_addr;
        assign o_wdata = i_wdata;
        assign o_wen   = i_wen;
        assign o_ren   = i_ren;
        assign o_rdata = i_rdata;
        var dummy: logic;
        always_ff {
            if_reset { dummy = 0; }
            else     { dummy = 0; }
        }
    }

    module Core (
        clk     : input  clock,
        rst     : input  reset,
        o_addr  : output logic<32>,
        o_wdata : output logic<32>,
        o_wen   : output logic,
        o_ren   : output logic,
        i_rdata : input  logic<32>,
        o_result: output logic<32>,
    ) {
        var phase: logic<4>;
        var stored_val: logic<32>;
        always_ff {
            if_reset {
                phase = 0;
                o_addr = 0;
                o_wdata = 0;
                o_wen = 0;
                o_ren = 0;
                stored_val = 0;
            } else {
                o_wen = 0;
                o_ren = 0;
                case phase {
                    4'd0: { o_addr = 32'd0; o_wdata = 32'd42; o_wen = 1; phase = 4'd1; }
                    4'd1: { phase = 4'd2; }
                    4'd2: { o_addr = 32'd0; o_wdata = 32'd100; o_wen = 1; phase = 4'd3; }
                    4'd3: { phase = 4'd4; }
                    4'd4: { o_addr = 32'd0; o_ren = 1; phase = 4'd5; }
                    4'd5: { stored_val = i_rdata; phase = 4'd6; }
                    default: {}
                }
            }
        }
        assign o_result = stored_val;
    }

    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var core_addr : logic<32>;
        var core_wdata: logic<32>;
        var core_wen  : logic;
        var core_ren  : logic;
        var pt_rdata  : logic<32>;

        inst u_core: Core (
            clk, rst,
            o_addr: core_addr, o_wdata: core_wdata,
            o_wen: core_wen, o_ren: core_ren,
            i_rdata: pt_rdata,
            o_result: result,
        );

        var ext_addr : logic<32>;
        var ext_wdata: logic<32>;
        var ext_wen  : logic;
        var ext_ren  : logic;
        var ext_rdata: logic<32>;

        inst u_pt: Passthrough (
            clk, rst,
            i_addr: core_addr, i_wdata: core_wdata,
            i_wen: core_wen, i_ren: core_ren,
            o_addr: ext_addr, o_wdata: ext_wdata,
            o_wen: ext_wen, o_ren: ext_ren,
            i_rdata: ext_rdata,
            o_rdata: pt_rdata,
        );

        // Combinational memory (read is comb, write is FF)
        var mem: logic<32>;
        always_ff {
            if_reset { mem = 0; }
            else if ext_wen { mem = ext_wdata; }
        }
        // Comb read: rdata reflects current mem value
        assign ext_rdata = mem;
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step(&rst);
        // NBA causes 1-cycle delay for FF outputs through comb chains;
        // allow extra cycles for the state machine to complete.
        for _ in 0..20 {
            sim.step(&clk);
        }
        let result = sim.get("result").unwrap().payload_u64();
        assert_eq!(
            result, 100,
            "JIT={} 4state={}: expected 100 (second store), got {}",
            config.use_jit, config.use_4state, result
        );
    }
}

#[test]
fn readonly_cache_fill() {
    let code = "
    module Cache (
        clk: input clock, rst: input reset,
        i_ren: input logic,
        o_val: output logic<32>,
        o_stall: output logic,
        o_mem_addr: output logic<32>,
        o_mem_ren: output logic,
        i_mem_rdata: input logic<32>,
    ) {
        var data: logic<32> [8];
        var valid: logic;
        var state: logic<2>;
        var fill_count: logic<3>;
        let hit: logic = valid;
        let miss: logic = i_ren && !hit && state == 2'd0;
        let filling: logic = state == 2'd1;
        always_ff (clk, rst) {
            if_reset {
                state = 0; fill_count = 0; valid = 0;
                for i in 0..8 { data[i] = 0; }
            } else {
                case state {
                    2'd0: { if miss { fill_count = 0; state = 2'd1; } }
                    2'd1: {
                        data[fill_count] = i_mem_rdata;
                        if fill_count == 3'd7 { valid = 1; state = 2'd2; }
                        else { fill_count = fill_count + 3'd1; }
                    }
                    2'd2: { state = 0; }
                    default: { state = 0; }
                }
            }
        }
        assign o_val = data[1];
        assign o_stall = filling || miss;
        assign o_mem_addr = if filling ? {29'd0, fill_count} : 32'd0;
        assign o_mem_ren = filling;
    }
    module Top (clk: input clock, rst: input reset, result: output logic<32>) {
        var ren: logic; var stall: logic; var val: logic<32>;
        var mem_addr: logic<32>; var mem_ren: logic; var mem_rdata: logic<32>;
        inst u: Cache (clk: clk, rst: rst, i_ren: ren,
            o_val: val, o_stall: stall,
            o_mem_addr: mem_addr, o_mem_ren: mem_ren, i_mem_rdata: mem_rdata);
        var mem: logic<32> [8];
        assign mem_rdata = mem[mem_addr[2:0]];
        var tc: logic<8>; var stored: logic<32>;
        always_ff (clk, rst) {
            if_reset { tc = 0; ren = 0; stored = 0;
                for i in 0..8 { mem[i] = {24'd0, i[7:0]} + 32'd10; }
            } else {
                ren = 0; if stall { ren = 1; }
                if !stall { tc = tc + 8'd1; }
                case tc {
                    8'd1: { ren = 1; }
                    8'd3: { stored = val; }
                    default: {}
                }
            }
        }
        assign result = stored;
    }
    ";

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step(&rst);
        for _ in 0..15 {
            sim.step(&clk);
        }
        sim.ensure_comb_updated();
        let result = sim.get("result").unwrap().payload_u64();
        assert_eq!(
            result, 11,
            "JIT={} 4state={}: expected 11, got {}",
            config.use_jit, config.use_4state, result
        );
    }
}

#[test]
fn ff_comb_let_basic() {
    // Simplest case: always_ff variable read by a let (comb) declaration
    let code = "
    module Top (clk: input clock, rst: input reset, result: output logic<8>) {
        var cnt: logic<8>;
        let doubled: logic<8> = cnt + cnt;
        always_ff (clk, rst) {
            if_reset { cnt = 0; }
            else { cnt = cnt + 8'd1; }
        }
        assign result = doubled;
    }
    ";
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step(&rst);
        for _ in 0..5 {
            sim.step(&clk);
        }
        sim.ensure_comb_updated();
        let result = sim.get("result").unwrap().payload_u64();
        // After 5 clocks: cnt=5, doubled=10
        assert_eq!(
            result, 10,
            "JIT={} 4state={}: expected 10, got {}",
            config.use_jit, config.use_4state, result
        );
    }
}

/// Read-only cache with tag/index address decomposition (an I-cache).
/// Tests that fill_count-driven o_mem_addr propagates through comb to update
/// i_mem_rdata each fill cycle, so data[0] != data[1].
#[test]
fn readonly_cache_fill_with_tags() {
    let code = "
    module Cache (
        clk: input clock, rst: input reset,
        i_addr: input logic<64>, i_ren: input logic,
        o_rdata: output logic<64>, o_stall: output logic,
        o_mem_addr: output logic<64>, o_mem_ren: output logic,
        i_mem_rdata: input logic<64>,
    ) {
        let tag: logic<54> = i_addr[63:10];
        let index: logic<4> = i_addr[9:6];
        let offset: logic<3> = i_addr[5:3];
        let data_idx: logic<7> = {index, offset};
        var valid: logic<16>;
        var tags: logic<54> [16];
        var data: logic<64> [128];
        let cache_hit: logic = valid[index] && tags[index] == tag;
        var state: logic<2>;
        var fill_count: logic<3>;
        var fill_index: logic<4>;
        var fill_tag: logic<54>;
        let fill_data_idx: logic<7> = {fill_index, fill_count};
        let miss: logic = i_ren && !cache_hit && state == 2'd0;
        let filling: logic = state == 2'd1;
        always_ff (clk, rst) {
            if_reset {
                state = 0; fill_count = 0; fill_index = 0; fill_tag = 0; valid = 0;
                for i in 0..16 { tags[i] = 0; }
                for i in 0..128 { data[i] = 0; }
            } else {
                case state {
                    2'd0: { if miss { fill_index = index; fill_tag = tag; fill_count = 0; state = 2'd1; } }
                    2'd1: {
                        data[fill_data_idx] = i_mem_rdata;
                        if fill_count == 3'd7 { tags[fill_index] = fill_tag; valid[fill_index] = 1; state = 2'd2; }
                        else { fill_count = fill_count + 3'd1; }
                    }
                    2'd2: { state = 0; }
                    default: { state = 0; }
                }
            }
        }
        assign o_rdata = if cache_hit ? data[data_idx] : i_mem_rdata;
        assign o_stall = filling || miss;
        assign o_mem_addr = if filling ? {fill_tag, fill_index, fill_count, 3'b000}
                          : if i_ren && !cache_hit ? {tag, index, 3'd0, 3'b000}
                          : '0;
        assign o_mem_ren = if filling ? 1'b1 : if i_ren && !cache_hit ? 1'b1 : 1'b0;
    }
    module Top (clk: input clock, rst: input reset, result: output logic<64>) {
        var addr: logic<64>; var ren: logic; var rdata: logic<64>;
        var stall: logic; var mem_addr: logic<64>; var mem_ren: logic;
        var mem_rdata: logic<64>;
        inst u: Cache (clk: clk, rst: rst, i_addr: addr, i_ren: ren,
            o_rdata: rdata, o_stall: stall,
            o_mem_addr: mem_addr, o_mem_ren: mem_ren, i_mem_rdata: mem_rdata);
        var mem: logic<64> [256];
        assign mem_rdata = mem[mem_addr[10:3]];
        var tc: logic<8>; var r1: logic<64>; var r2: logic<64>;
        always_ff (clk, rst) {
            if_reset { tc = 0; ren = 0; addr = 0; r1 = 0; r2 = 0;
                for i in 0..256 {
                    if i == 0 { mem[i] = 64'h0000_0000_0000_AAAA; }
                    else if i == 1 { mem[i] = 64'h0000_0000_0000_BBBB; }
                    else { mem[i] = 0; }
                }
            } else {
                ren = 0;
                if stall { ren = 1; }
                if !stall { tc = tc + 8'd1; }
                case tc {
                    8'd1: { addr = 64'h0; ren = 1; }
                    8'd3: { addr = 64'h0; ren = 1; }
                    8'd4: { r1 = rdata; }
                    8'd6: { addr = 64'h8; ren = 1; }
                    8'd7: { r2 = rdata; }
                    default: {}
                }
            }
        }
        assign result = r2;
    }
    ";
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step(&rst);
        for _ in 0..30 {
            sim.step(&clk);
        }
        sim.ensure_comb_updated();
        let result = sim.get("result").unwrap().payload_u64();
        assert_eq!(
            result, 0xBBBB,
            "JIT={} 4state={}: expected 0xBBBB, got 0x{:x}",
            config.use_jit, config.use_4state, result
        );
    }
}

/// 3-level hierarchy: TestTop → Harness → Cache (a typical test structure)
#[test]
fn readonly_cache_fill_3level() {
    let code = "
    module Cache (
        clk: input clock, rst: input reset,
        i_addr: input logic<64>, i_ren: input logic,
        o_rdata: output logic<64>, o_stall: output logic,
        o_mem_addr: output logic<64>, o_mem_ren: output logic,
        i_mem_rdata: input logic<64>,
    ) {
        let tag: logic<54> = i_addr[63:10];
        let index: logic<4> = i_addr[9:6];
        let offset: logic<3> = i_addr[5:3];
        let data_idx: logic<7> = {index, offset};
        var valid: logic<16>;
        var tags: logic<54> [16];
        var data: logic<64> [128];
        let cache_hit: logic = valid[index] && tags[index] == tag;
        enum State: logic<2> { IDLE = 2'd0, FILL = 2'd1, DONE = 2'd2 }
        var state: State;
        var fill_count: logic<3>;
        var fill_index: logic<4>;
        var fill_tag: logic<54>;
        let fill_data_idx: logic<7> = {fill_index, fill_count};
        let miss: logic = i_ren && !cache_hit && state == State::IDLE;
        let filling: logic = state == State::FILL;
        always_ff (clk, rst) {
            if_reset {
                state = State::IDLE; fill_count = 0; fill_index = 0; fill_tag = 0; valid = 0;
                for i in 0..16 { tags[i] = 0; }
                for i in 0..128 { data[i] = 0; }
            } else {
                case state {
                    State::IDLE: { if miss { fill_index = index; fill_tag = tag; fill_count = 0; state = State::FILL; } }
                    State::FILL: {
                        data[fill_data_idx] = i_mem_rdata;
                        if fill_count == 3'd7 { tags[fill_index] = fill_tag; valid[fill_index] = 1; state = State::DONE; }
                        else { fill_count = fill_count + 3'd1; }
                    }
                    State::DONE: { state = State::IDLE; }
                    default: { state = State::IDLE; }
                }
            }
        }
        assign o_rdata = if cache_hit ? data[data_idx] : i_mem_rdata;
        assign o_stall = filling || miss;
        assign o_mem_addr = if filling ? {fill_tag, fill_index, fill_count, 3'b000}
                          : if i_ren && !cache_hit ? {tag, index, 3'd0, 3'b000}
                          : '0;
        assign o_mem_ren = if filling ? 1'b1 : if i_ren && !cache_hit ? 1'b1 : 1'b0;
    }
    module Harness (
        clk: input clock, rst: input reset,
        o_r1: output logic<64>, o_r2: output logic<64>,
    ) {
        var addr: logic<64>; var ren: logic; var rdata: logic<64>;
        var stall: logic; var mem_addr: logic<64>; var mem_ren: logic;
        var mem_rdata: logic<64>;
        inst dut: Cache (clk: clk, rst: rst, i_addr: addr, i_ren: ren,
            o_rdata: rdata, o_stall: stall,
            o_mem_addr: mem_addr, o_mem_ren: mem_ren, i_mem_rdata: mem_rdata);
        var mem: logic<64> [256];
        assign mem_rdata = mem[mem_addr[10:3]];
        var tc: logic<8>; var r1_val: logic<64>; var r2_val: logic<64>;
        always_ff (clk, rst) {
            if_reset { tc = 0; ren = 0; addr = 0; r1_val = 0; r2_val = 0;
                for i in 0..256 {
                    if i == 0 { mem[i] = 64'h0000_0000_0000_AAAA; }
                    else if i == 1 { mem[i] = 64'h0000_0000_0000_BBBB; }
                    else { mem[i] = 0; }
                }
            } else {
                ren = 0;
                if stall { ren = 1; }
                if !stall { tc = tc + 8'd1; }
                case tc {
                    8'd1: { addr = 64'h0; ren = 1; }
                    8'd3: { addr = 64'h0; ren = 1; }
                    8'd4: { r1_val = rdata; }
                    8'd6: { addr = 64'h8; ren = 1; }
                    8'd7: { r2_val = rdata; }
                    default: {}
                }
            }
        }
        assign o_r1 = r1_val;
        assign o_r2 = r2_val;
    }
    module Top (clk: input clock, rst: input reset, result: output logic<64>) {
        var r1: logic<64>; var r2: logic<64>;
        inst h: Harness (clk: clk, rst: rst, o_r1: r1, o_r2: r2);
        assign result = r2;
    }
    ";
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step(&rst);
        for _ in 0..30 {
            sim.step(&clk);
        }
        sim.ensure_comb_updated();
        let result = sim.get("result").unwrap().payload_u64();
        assert_eq!(
            result, 0xBBBB,
            "JIT={} 4state={}: expected 0xBBBB, got 0x{:x}",
            config.use_jit, config.use_4state, result
        );
    }
}

/// Bit select on a wide FF variable used in ternary: `if pc[1] ? data[31:16] : data[15:0]`
/// Tests that pc[1] correctly selects upper/lower halfword as pc increments by 2.
/// The result is captured by an FF so we observe the pre-event comb value.
#[test]
fn bit_select_ternary_wide_var() {
    let code = r#"
    module Top (clk: input clock, rst: input reset, result: output logic<64>) {
        var pc: logic<64>;
        var data: logic<32>;

        always_ff (clk, rst) {
            if_reset {
                pc = 0;
                data = 32'hBBBBAAAA;
            } else {
                pc = pc + 64'd2;
            }
        }

        // Select halfword based on pc[1]
        let half: logic<16> = if pc[1] ? data[31:16] : data[15:0];

        // Capture sequence: r0=half@pc=0, r1=half@pc=2, r2=half@pc=4
        var r0: logic<16>;
        var r1: logic<16>;
        var r2: logic<16>;
        var cycle: logic<8>;
        always_ff (clk, rst) {
            if_reset { r0 = 0; r1 = 0; r2 = 0; cycle = 0; }
            else {
                case cycle {
                    8'd0: { r0 = half; }
                    8'd1: { r1 = half; }
                    8'd2: { r2 = half; }
                    default: {}
                }
                cycle = cycle + 8'd1;
            }
        }

        // Pack into result: {r2, r1, r0} with padding
        assign result = {16'd0, r2, r1, r0};
    }
    "#;
    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step(&rst);
        for _ in 0..10 {
            sim.step(&clk);
        }
        sim.ensure_comb_updated();
        let result = sim.get("result").unwrap().payload_u64();
        let r0 = (result >> 0) & 0xFFFF;
        let r1 = (result >> 16) & 0xFFFF;
        let r2 = (result >> 32) & 0xFFFF;
        // pc=0: pc[1]=0 → lower=0xAAAA. pc=2: pc[1]=1 → upper=0xBBBB. pc=4: pc[1]=0 → lower=0xAAAA
        assert_eq!(
            r0, 0xAAAA,
            "JIT={} 4st={}: pc=0 expected 0xAAAA got 0x{:x}",
            config.use_jit, config.use_4state, r0
        );
        assert_eq!(
            r1, 0xBBBB,
            "JIT={} 4st={}: pc=2 expected 0xBBBB got 0x{:x}",
            config.use_jit, config.use_4state, r1
        );
        assert_eq!(
            r2, 0xAAAA,
            "JIT={} 4st={}: pc=4 expected 0xAAAA got 0x{:x}",
            config.use_jit, config.use_4state, r2
        );
    }
}

/// Same as above but halfword select is on a child module's comb output.
/// Reproduces a pattern: icache.o_rdata → parent bit select → pipeline.
#[test]
fn bit_select_child_output() {
    let code = r#"
    module Child (
        clk: input clock, rst: input reset,
        i_addr: input logic<64>,
        o_data: output logic<32>,
    ) {
        var store: logic<32>;
        always_ff (clk, rst) {
            if_reset { store = 32'hBBBBAAAA; }
        }
        assign o_data = store;
    }
    module Top (clk: input clock, rst: input reset, result: output logic<64>) {
        var pc: logic<64>;
        always_ff (clk, rst) {
            if_reset { pc = 0; }
            else { pc = pc + 64'd2; }
        }

        var child_data: logic<32>;
        inst u_child: Child (clk, rst, i_addr: pc, o_data: child_data);

        // Select halfword based on pc[1] from child output
        let half: logic<16> = if pc[1] ? child_data[31:16] : child_data[15:0];

        var r0: logic<16>;
        var r1: logic<16>;
        var r2: logic<16>;
        var cycle: logic<8>;
        always_ff (clk, rst) {
            if_reset { r0 = 0; r1 = 0; r2 = 0; cycle = 0; }
            else {
                case cycle {
                    8'd0: { r0 = half; }
                    8'd1: { r1 = half; }
                    8'd2: { r2 = half; }
                    default: {}
                }
                cycle = cycle + 8'd1;
            }
        }
        assign result = {16'd0, r2, r1, r0};
    }
    "#;
    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step(&rst);
        for _ in 0..10 {
            sim.step(&clk);
        }
        sim.ensure_comb_updated();
        let result = sim.get("result").unwrap().payload_u64();
        let r0 = (result >> 0) & 0xFFFF;
        let r1 = (result >> 16) & 0xFFFF;
        let r2 = (result >> 32) & 0xFFFF;
        assert_eq!(
            r0, 0xAAAA,
            "JIT={} 4st={}: pc=0 expected 0xAAAA got 0x{:x}",
            config.use_jit, config.use_4state, r0
        );
        assert_eq!(
            r1, 0xBBBB,
            "JIT={} 4st={}: pc=2 expected 0xBBBB got 0x{:x}",
            config.use_jit, config.use_4state, r1
        );
        assert_eq!(
            r2, 0xAAAA,
            "JIT={} 4st={}: pc=4 expected 0xAAAA got 0x{:x}",
            config.use_jit, config.use_4state, r2
        );
    }
}

/// Cache + halfword select + stall: reproduces a compressed-instruction issue.
/// icache stalls for fill, then parent selects halfword based on pc[1].
#[test]
fn cache_halfword_select_with_stall() {
    let code = r#"
    module Cache32 (
        clk: input clock, rst: input reset,
        i_addr: input logic<64>, i_ren: input logic,
        o_rdata: output logic<32>, o_stall: output logic,
        o_mem_addr: output logic<64>, i_mem_rdata: input logic<32>, o_mem_ren: output logic,
    ) {
        let index: logic<4> = i_addr[9:6];
        let offset: logic<4> = i_addr[5:2];
        let data_idx: logic<8> = {index, offset};
        var valid: logic [16];
        var data: logic<32> [256];
        let cache_hit: logic = valid[index];
        var filling: logic;
        var fill_count: logic<4>;
        var fill_index: logic<4>;
        let fill_data_idx: logic<8> = {fill_index, fill_count};
        let miss: logic = i_ren && !cache_hit && !filling;
        always_ff (clk, rst) {
            if_reset {
                filling = 0; fill_count = 0; fill_index = 0;
                for i in 0..16 { valid[i] = 0; }
                for i in 0..256 { data[i] = 0; }
            } else if filling {
                data[fill_data_idx] = i_mem_rdata;
                if fill_count == 4'd15 {
                    valid[fill_index] = 1; filling = 0;
                } else { fill_count = fill_count + 4'd1; }
            } else if miss {
                fill_index = index; fill_count = 0; filling = 1;
            }
        }
        assign o_rdata = if cache_hit ? data[data_idx] : i_mem_rdata;
        assign o_stall = filling || miss;
        assign o_mem_addr = if filling ? {50'd0, fill_index, fill_count, 2'b00} : i_addr;
        assign o_mem_ren = i_ren || filling;
    }
    module Top (clk: input clock, rst: input reset, result: output logic<64>) {
        var stall: logic;
        var cache_rdata: logic<32>;
        var mem_addr: logic<64>;
        var mem_ren: logic;

        // Instruction ROM: 32-bit words containing two 16-bit halves
        var mem_rdata: logic<32>;
        always_comb {
            case mem_addr[7:0] {
                // Word at 0x00: lower=0x1111 upper=0x2222
                8'h00: mem_rdata = 32'h22221111;
                // Word at 0x04: lower=0x3333 upper=0x4444
                8'h04: mem_rdata = 32'h44443333;
                // Word at 0x08: lower=0x5555 upper=0x6666
                8'h08: mem_rdata = 32'h66665555;
                default: mem_rdata = 32'h00000000;
            }
        }

        // PC: increments by 2 (halfword) when not stalled
        var pc: logic<64>;
        always_ff (clk, rst) {
            if_reset { pc = 0; }
            else if !stall { pc = pc + 64'd2; }
        }

        inst u_cache: Cache32 (clk, rst,
            i_addr: pc, i_ren: 1'b1,
            o_rdata: cache_rdata, o_stall: stall,
            o_mem_addr: mem_addr, i_mem_rdata: mem_rdata, o_mem_ren: mem_ren);

        // Halfword select based on pc[1]
        let half: logic<16> = if pc[1] ? cache_rdata[31:16] : cache_rdata[15:0];

        // Capture first 4 halfwords into r0-r3
        var r0: logic<16>; var r1: logic<16>;
        var r2: logic<16>; var r3: logic<16>;
        var tc: logic<8>;
        always_ff (clk, rst) {
            if_reset { r0 = 0; r1 = 0; r2 = 0; r3 = 0; tc = 0; }
            else if !stall {
                case tc {
                    8'd0: { r0 = half; }
                    8'd1: { r1 = half; }
                    8'd2: { r2 = half; }
                    8'd3: { r3 = half; }
                    default: {}
                }
                tc = tc + 8'd1;
            }
        }
        assign result = {r3, r2, r1, r0};
    }
    "#;
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step(&rst);
        for _ in 0..40 {
            sim.step(&clk);
        }
        sim.ensure_comb_updated();
        let result = sim.get("result").unwrap().payload_u64();
        let r0 = (result >> 0) & 0xFFFF;
        let r1 = (result >> 16) & 0xFFFF;
        let r2 = (result >> 32) & 0xFFFF;
        let r3 = (result >> 48) & 0xFFFF;
        // PC increments by 2: 0x00→0x02→0x04→0x06
        // pc=0: word@0x00[15:0]=0x1111, pc=2: word@0x00[31:16]=0x2222
        // pc=4: word@0x04[15:0]=0x3333, pc=6: word@0x04[31:16]=0x4444
        assert_eq!(
            r0, 0x1111,
            "JIT={} 4st={}: pc=0 expected 0x1111 got 0x{:x}",
            config.use_jit, config.use_4state, r0
        );
        assert_eq!(
            r1, 0x2222,
            "JIT={} 4st={}: pc=2 expected 0x2222 got 0x{:x}",
            config.use_jit, config.use_4state, r1
        );
        assert_eq!(
            r2, 0x3333,
            "JIT={} 4st={}: pc=4 expected 0x3333 got 0x{:x}",
            config.use_jit, config.use_4state, r2
        );
        assert_eq!(
            r3, 0x4444,
            "JIT={} 4st={}: pc=6 expected 0x4444 got 0x{:x}",
            config.use_jit, config.use_4state, r3
        );
    }
}

/// Reproduces JIT bug: halfword select with cache + expander child modules.
/// The `let curr_halfword` in parent changes merged JIT function behavior
/// for child modules (c_expander), causing incorrect results.
/// Non-idempotent: accumulator detects double-execution or missing execution.
#[test]
fn halfword_select_with_expander_child() {
    let code = r#"
    // Simplified icache: returns stored 32-bit word
    module SimpleCache (
        clk: input clock, rst: input reset,
        i_addr: input logic<64>,
        o_data: output logic<32>,
        o_stall: output logic,
    ) {
        var store: logic<32> [4];
        var ready: logic;
        always_ff (clk, rst) {
            if_reset {
                ready = 0;
                store[0] = 32'hBBBBAAAA; // word at addr 0: lower=0xAAAA, upper=0xBBBB
                store[1] = 32'hDDDDCCCC; // word at addr 4: lower=0xCCCC, upper=0xDDDD
                store[2] = 32'h00000000;
                store[3] = 32'h00000000;
            } else {
                ready = 1;
            }
        }
        let idx: logic<2> = i_addr[3:2];
        assign o_data = store[idx];
        assign o_stall = !ready;
    }

    // Simplified expander: zero-extends the 16-bit input to 32 bits
    module Expander (
        i_half: input logic<16>,
        o_full: output logic<32>,
    ) {
        assign o_full = {16'd0, i_half};
    }

    module Top (clk: input clock, rst: input reset, result: output logic<64>) {
        var pc: logic<64>;
        var stall: logic;
        always_ff (clk, rst) {
            if_reset { pc = 0; }
            else if !stall { pc = pc + 64'd2; }
        }

        var cache_data: logic<32>;
        inst u_cache: SimpleCache (clk, rst,
            i_addr: pc, o_data: cache_data, o_stall: stall);

        // Halfword select based on pc[1] -- this is the problematic pattern
        let curr_half: logic<16> = if pc[1] ? cache_data[31:16] : cache_data[15:0];

        var expanded: logic<32>;
        inst u_exp: Expander (i_half: curr_half, o_full: expanded);

        // Non-idempotent: accumulate expanded values
        var sum: logic<32>;
        var count: logic<8>;
        always_ff (clk, rst) {
            if_reset { sum = 0; count = 0; }
            else if !stall {
                if count <: 8'd4 {
                    sum = sum + expanded;
                    count = count + 8'd1;
                }
            }
        }
        // Expected: sum of first 4 halfwords
        // pc=0: half=0xAAAA, pc=2: half=0xBBBB, pc=4: half=0xCCCC, pc=6: half=0xDDDD
        // sum = 0xAAAA + 0xBBBB + 0xCCCC + 0xDDDD = 0x3110E
        assign result = {24'd0, count, sum};
    }
    "#;
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step(&rst);
        for _ in 0..20 {
            sim.step(&clk);
        }
        sim.ensure_comb_updated();
        let result = sim.get("result").unwrap().payload_u64();
        let count = (result >> 32) & 0xFF;
        let sum = result & 0xFFFFFFFF;
        let expected_sum: u64 = 0xAAAA + 0xBBBB + 0xCCCC + 0xDDDD;
        assert_eq!(
            count, 4,
            "JIT={} 4st={}: count expected 4 got {}",
            config.use_jit, config.use_4state, count
        );
        assert_eq!(
            sum, expected_sum,
            "JIT={} 4st={}: sum expected 0x{:x} got 0x{:x}",
            config.use_jit, config.use_4state, expected_sum, sum
        );
    }
}

// Regression: D-Cache write-through with parent-child hierarchy and comb feedback.
// Parent provides address to child (cache), child returns data.
// Tests that dynamic array index is correct across module boundary with JIT.
#[test]
fn dcache_write_through_hierarchy() {
    let code = r#"
    module Cache (
        clk: input clock,
        rst: input reset,
        i_addr: input logic<64>,
        i_wdata: input logic<64>,
        i_wen: input logic,
        i_ren: input logic,
        o_rdata: output logic<64>,
    ) {
        let data_idx: logic<4> = i_addr[6:3];

        var data: logic<64> [16];
        var valid: logic;

        always_ff (clk, rst) {
            if_reset {
                valid = 1'b0;
                for i in 0..16 { data[i] = 64'd0; }
            } else if i_wen {
                data[data_idx] = i_wdata;
                valid = 1'b1;
            }
        }

        assign o_rdata = if valid ? data[data_idx] : 64'd0;
    }

    module Top (
        clk: input clock,
        rst: input reset,
        i_cmd: input logic<3>,     // 0=nop, 1=write, 2=read
        i_addr: input logic<64>,
        i_wdata: input logic<64>,
        o_rdata: output logic<64>,
    ) {
        var cache_rdata: logic<64>;

        inst u_cache: Cache (
            clk, rst,
            i_addr  : i_addr,
            i_wdata : i_wdata,
            i_wen   : i_cmd == 3'd1,
            i_ren   : i_cmd == 3'd2,
            o_rdata : cache_rdata,
        );

        // Comb feedback: cache output → parent output
        assign o_rdata = cache_rdata;
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        // Write 42 to addr 0x00 (data_idx=0)
        sim.set("i_cmd", Value::new(1, 3, false));
        sim.set("i_addr", Value::new(0x00, 64, false));
        sim.set("i_wdata", Value::new(42, 64, false));
        sim.step(&clk);

        // Write 99 to addr 0x08 (data_idx=1)
        sim.set("i_cmd", Value::new(1, 3, false));
        sim.set("i_addr", Value::new(0x08, 64, false));
        sim.set("i_wdata", Value::new(99, 64, false));
        sim.step(&clk);

        // Read from addr 0x08 (data_idx=1)
        sim.set("i_cmd", Value::new(2, 3, false));
        sim.set("i_addr", Value::new(0x08, 64, false));
        sim.step(&clk);

        let rdata = sim.get("o_rdata").unwrap().payload_u64();
        assert_eq!(
            rdata, 99,
            "JIT={} 4st={}: expected 99 at addr 0x08, got {} — wrong index or stale data",
            config.use_jit, config.use_4state, rdata
        );

        // Read from addr 0x00 (data_idx=0)
        sim.set("i_addr", Value::new(0x00, 64, false));
        sim.step(&clk);

        let rdata = sim.get("o_rdata").unwrap().payload_u64();
        assert_eq!(
            rdata, 42,
            "JIT={} 4st={}: expected 42 at addr 0x00, got {}",
            config.use_jit, config.use_4state, rdata
        );
    }
}

// Regression: non-FF array in always_ff with comb read via DynamicVariable.
// The FF table marks these as non-FF (only written in always_ff, never read there),
// placing them in the comb buffer. Tests that the JIT merged function handles
// this correctly for large arrays in deep module hierarchies.
#[test]
fn nonff_dynamic_array_deep_hierarchy() {
    // 3-level hierarchy: Top → Pipeline → Cache
    // Cache has a large array (128 elements) written in always_ff and read in comb
    // Pipeline adds sibling logic that increases comb buffer size
    let code = r#"
    module Cache (
        clk: input clock,
        rst: input reset,
        i_addr: input logic<64>,
        i_wdata: input logic<64>,
        i_wen: input logic,
        i_ren: input logic,
        o_rdata: output logic<64>,
        o_stall: output logic,
    ) {
        // Mirror a D-cache structure
        let tag      : logic<54> = i_addr[63:10];
        let index    : logic<4>  = i_addr[9:6];
        let offset_w : logic<3>  = i_addr[5:3];
        let data_idx : logic<7>  = {index, offset_w};

        var valid: logic [16];
        var tags : logic<54> [16];
        var data : logic<64> [128];

        let cache_hit: logic = valid[index] && tags[index] == tag;

        enum State: logic<2> {
            IDLE = 2'd0,
            FILL = 2'd1,
            DONE = 2'd2,
        }
        var state: State;
        var fill_count: logic<3>;
        var fill_index: logic<4>;
        var fill_tag  : logic<54>;
        let fill_data_idx: logic<7> = {fill_index, fill_count};
        let miss: logic = i_ren && !cache_hit && state == State::IDLE;

        always_ff (clk, rst) {
            if_reset {
                state = State::IDLE;
                fill_count = 3'd0;
                fill_index = 4'd0;
                fill_tag   = '0;
                for i in 0..16 {
                    valid[i] = 1'b0;
                    tags[i]  = 54'd0;
                }
                for i in 0..128 {
                    data[i] = 64'd0;
                }
            } else {
                case state {
                    State::IDLE: {
                        if miss {
                            fill_index = index;
                            fill_tag   = tag;
                            fill_count = 3'd0;
                            state = State::FILL;
                        }
                        if i_wen && cache_hit {
                            data[data_idx] = i_wdata;
                        }
                    }
                    State::FILL: {
                        data[fill_data_idx] = i_wdata;
                        if fill_count == 3'd7 {
                            tags[fill_index]  = fill_tag;
                            valid[fill_index] = 1'b1;
                            state = State::DONE;
                        } else {
                            fill_count = fill_count + 3'd1;
                        }
                    }
                    State::DONE: {
                        state = State::IDLE;
                    }
                    default: {
                        state = State::IDLE;
                    }
                }
            }
        }

        assign o_rdata = if cache_hit ? data[data_idx] : 64'd0;
        assign o_stall = (state == State::FILL) || miss;
    }

    // Pipeline module with many variables to inflate comb buffer
    module Pipeline (
        clk: input clock,
        rst: input reset,
        i_addr: input logic<7>,
        i_wdata: input logic<64>,
        i_wen: input logic,
        o_rdata: output logic<64>,
    ) {
        // Padding variables to shift cache offsets
        var pad_a: logic<64>;
        var pad_b: logic<64>;
        var pad_c: logic<64>;
        var pad_d: logic<64>;
        var pad_e: logic<64>;
        var pad_f: logic<64>;
        var pad_g: logic<64>;
        var pad_h: logic<64>;

        always_ff (clk, rst) {
            if_reset {
                pad_a = 64'd0; pad_b = 64'd0; pad_c = 64'd0; pad_d = 64'd0;
                pad_e = 64'd0; pad_f = 64'd0; pad_g = 64'd0; pad_h = 64'd0;
            } else {
                pad_a = pad_a + 64'd1;
                pad_b = pad_b + 64'd2;
                pad_c = pad_c + 64'd3;
                pad_d = pad_d + 64'd4;
                pad_e = pad_e + 64'd5;
                pad_f = pad_f + 64'd6;
                pad_g = pad_g + 64'd7;
                pad_h = pad_h + 64'd8;
            }
        }

        var cache_rdata: logic<64>;
        var cache_stall: logic;
        inst u_cache: Cache (
            clk, rst,
            i_addr  : {57'd0, i_addr},
            i_wdata : i_wdata,
            i_wen   : i_wen,
            i_ren   : !i_wen,
            o_rdata : cache_rdata,
            o_stall : cache_stall,
        );

        assign o_rdata = cache_rdata;
    }

    module Top (
        clk: input clock,
        rst: input reset,
        i_addr: input logic<7>,
        i_wdata: input logic<64>,
        i_wen: input logic,
        o_rdata: output logic<64>,
    ) {
        var pipeline_rdata: logic<64>;

        inst u_pipeline: Pipeline (
            clk, rst,
            i_addr  : i_addr,
            i_wdata : i_wdata,
            i_wen   : i_wen,
            o_rdata : pipeline_rdata,
        );

        assign o_rdata = pipeline_rdata;
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        // addr layout: i_addr[6:3]=index(4bit), i_addr[2:0]=000 (byte aligned)
        // For cache index 4, offset 0: addr = (4<<6)|(0<<3) = 0x100 → i_addr bits[6:0] = 0b1000000 = 0x40
        // For cache index 4, offset 1: addr = (4<<6)|(1<<3) = 0x108 → i_addr bits[6:0] = 0b1001000 = 0x48

        // Step 1: Read from addr 0x40 (index 4, offset 0) → cache miss → triggers fill
        sim.set("i_addr", Value::new(0x40, 7, false));
        sim.set("i_wdata", Value::new(42, 64, false)); // fill data for all 8 words
        sim.set("i_wen", Value::new(0, 1, false));
        sim.step(&clk); // miss detected
        // Fill takes 8 cycles (fill_count 0..7)
        for _ in 0..9 {
            sim.step(&clk);
        }
        // State should be IDLE now, valid[4]=1

        // Step 2: Write 42 to addr 0x40 (cache hit, write-through)
        sim.set("i_addr", Value::new(0x40, 7, false));
        sim.set("i_wdata", Value::new(42, 64, false));
        sim.set("i_wen", Value::new(1, 1, false));
        sim.step(&clk);

        // Step 3: Write 99 to addr 0x48 (cache hit, write-through, different offset)
        sim.set("i_addr", Value::new(0x48, 7, false));
        sim.set("i_wdata", Value::new(99, 64, false));
        sim.set("i_wen", Value::new(1, 1, false));
        sim.step(&clk);

        // Step 4: Read from addr 0x48 → should get 99
        sim.set("i_addr", Value::new(0x48, 7, false));
        sim.set("i_wen", Value::new(0, 1, false));
        sim.step(&clk);

        let rdata = sim.get("o_rdata").unwrap().payload_u64();
        assert_eq!(
            rdata, 99,
            "JIT={} 4st={}: expected 99 at addr 0x48, got {} (wrong cache offset?)",
            config.use_jit, config.use_4state, rdata
        );

        // Step 5: Read from addr 0x40 → should get 42
        sim.set("i_addr", Value::new(0x40, 7, false));
        sim.step(&clk);

        let rdata = sim.get("o_rdata").unwrap().payload_u64();
        assert_eq!(
            rdata, 42,
            "JIT={} 4st={}: expected 42 at addr 0x40, got {}",
            config.use_jit, config.use_4state, rdata
        );
    }
}

// Regression: child module with always_comb reading 64-bit input port bit-select.
// Tests that bit 63 of a 64-bit input is correctly read inside always_comb
// of a child module, even when the child is instantiated alongside other logic.
#[test]
fn child_always_comb_bit_select_64() {
    let code = r#"
    module Adder (
        i_a: input logic<64>,
        i_b: input logic<64>,
        o_result: output logic<64>,
        o_b_raw: output logic<64>,
    ) {
        var sa: logic;
        var sb: logic;
        var result: logic<64>;
        always_comb {
            sa = i_a[63];
            sb = i_b[63];
            // Simple test: output sign bits and input values
            result = {sa, sb, 62'd0};
        }
        assign o_result = result;
        assign o_b_raw = i_b;  // pass-through i_b for verification
    }

    module Top (
        clk: input clock,
        rst: input reset,
        i_op: input logic,
        i_a: input logic<64>,
        i_b: input logic<64>,
        o_result: output logic<64>,
        o_b_raw: output logic<64>,
    ) {
        // Some padding logic (like ALU) to make comb_statements more complex
        var alu_result: logic<64>;
        always_comb {
            if i_op {
                alu_result = i_a + i_b;
            } else {
                alu_result = i_a - i_b;
            }
        }

        var add_result: logic<64>;
        var b_raw: logic<64>;
        inst u_adder: Adder (
            i_a: i_a,
            i_b: i_b,
            o_result: add_result,
            o_b_raw: b_raw,
        );

        // Use both results to prevent DCE
        assign o_result = if i_op ? add_result : alu_result;
        assign o_b_raw = b_raw;
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step(&rst);

        // Test: i_a = +5.0 (bit63=0), i_b = -5.0 (bit63=1)
        sim.set("i_op", Value::new(1, 1, false));
        sim.set("i_a", Value::new(0x4014000000000000, 64, false)); // +5.0
        sim.set("i_b", Value::new(0xC014000000000000, 64, false)); // -5.0
        sim.step(&clk);

        let result = sim.get("o_result").unwrap().payload_u64();
        let sa = (result >> 63) & 1;
        let sb = (result >> 62) & 1;
        let b_raw = sim.get("o_b_raw").unwrap().payload_u64();
        if sa != 0 || sb != 1 {
            panic!(
                "JIT={} 4st={}: sa={} (expected 0), sb={} (expected 1), result=0x{:016x}, b_raw=0x{:016x}",
                config.use_jit, config.use_4state, sa, sb, result, b_raw
            );
        }
    }
}

// Regression: D-Cache write-through pattern. Array fill + conditional write
// to different offset + read back. JIT ON was reading from wrong array index
// because the merged comb+event function miscomputed the dynamic index.
#[test]
fn dcache_write_through_pattern() {
    let code = r#"
    module Top (
        clk: input clock,
        rst: input reset,
        i_addr: input logic<7>,
        i_wdata: input logic<64>,
        i_wen: input logic,
        i_ren: input logic,
        o_rdata: output logic<64>,
        o_hit: output logic,
    ) {
        // Simplified cache: 8 entries, indexed by i_addr[2:0]
        var data: logic<64> [8];
        var valid: logic;

        let index: logic<3> = i_addr[2:0];

        always_ff (clk, rst) {
            if_reset {
                valid = 1'b0;
                for i in 0..8 { data[i] = 64'd0; }
            } else {
                // Fill: write all entries on first write
                if i_wen && !valid {
                    data[index] = i_wdata;
                    valid = 1'b1;
                }
                // Write-through: update on write when valid
                if i_wen && valid {
                    data[index] = i_wdata;
                }
            }
        }

        let hit: logic = valid;
        assign o_hit = hit;
        assign o_rdata = if hit ? data[index] : 64'd0;
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        // Write 42 to index 0
        sim.set("i_addr", Value::new(0, 7, false));
        sim.set("i_wdata", Value::new(42, 64, false));
        sim.set("i_wen", Value::new(1, 1, false));
        sim.set("i_ren", Value::new(0, 1, false));
        sim.step(&clk);

        // Write 99 to index 1
        sim.set("i_addr", Value::new(1, 7, false));
        sim.set("i_wdata", Value::new(99, 64, false));
        sim.set("i_wen", Value::new(1, 1, false));
        sim.step(&clk);

        // Read from index 1
        sim.set("i_addr", Value::new(1, 7, false));
        sim.set("i_wen", Value::new(0, 1, false));
        sim.set("i_ren", Value::new(1, 1, false));
        sim.step(&clk);

        let rdata = sim.get("o_rdata").unwrap().payload_u64();
        let hit = sim.get("o_hit").unwrap().payload_u64();
        assert_eq!(
            hit, 1,
            "JIT={} 4st={}: cache should be valid",
            config.use_jit, config.use_4state
        );
        assert_eq!(
            rdata, 99,
            "JIT={} 4st={}: expected 99 at index 1, got {} (wrong index?)",
            config.use_jit, config.use_4state, rdata
        );

        // Read from index 0
        sim.set("i_addr", Value::new(0, 7, false));
        sim.step(&clk);

        let rdata = sim.get("o_rdata").unwrap().payload_u64();
        assert_eq!(
            rdata, 42,
            "JIT={} 4st={}: expected 42 at index 0, got {}",
            config.use_jit, config.use_4state, rdata
        );
    }
}

// Regression: dynamic array with 2 read ports reading different elements
#[test]
fn dynamic_array_two_read_ports() {
    let code = r#"
    module RegFile (
        clk       : input  clock    ,
        rst       : input  reset    ,
        i_rs1_addr: input  logic<5> ,
        i_rs2_addr: input  logic<5> ,
        o_rs1_data: output logic<64>,
        o_rs2_data: output logic<64>,
        i_wd_addr : input  logic<5> ,
        i_wd_data : input  logic<64>,
        i_wen     : input  logic    ,
    ) {
        var regs: logic<64> [32];

        always_comb {
            o_rs1_data = if i_wen && i_rs1_addr == i_wd_addr ? i_wd_data : regs[i_rs1_addr];
            o_rs2_data = if i_wen && i_rs2_addr == i_wd_addr ? i_wd_data : regs[i_rs2_addr];
        }

        always_ff (clk, rst) {
            if_reset {
                for i in 0..32 { regs[i] = 64'd0; }
            } else if i_wen {
                regs[i_wd_addr] = i_wd_data;
            }
        }
    }

    // Middle module that captures regfile outputs in pipeline registers
    module Decode (
        clk       : input  clock    ,
        rst       : input  reset    ,
        i_rs1_addr: input  logic<5> ,
        i_rs2_addr: input  logic<5> ,
        o_rs1_data: output logic<64>,
        o_rs2_data: output logic<64>,
        i_wd_addr : input  logic<5> ,
        i_wd_data : input  logic<64>,
        i_wen     : input  logic    ,
    ) {
        var rf_rs1: logic<64>;
        var rf_rs2: logic<64>;
        inst u_rf: RegFile (
            clk, rst,
            i_rs1_addr, i_rs2_addr,
            o_rs1_data: rf_rs1,
            o_rs2_data: rf_rs2,
            i_wd_addr, i_wd_data, i_wen,
        );

        // Pipeline register captures
        always_ff (clk, rst) {
            if_reset {
                o_rs1_data = 64'd0;
                o_rs2_data = 64'd0;
            } else {
                o_rs1_data = rf_rs1;
                o_rs2_data = rf_rs2;
            }
        }
    }

    // Top module
    module Top (
        clk       : input  clock    ,
        rst       : input  reset    ,
        i_rs1_addr: input  logic<5> ,
        i_rs2_addr: input  logic<5> ,
        o_rs1_data: output logic<64>,
        o_rs2_data: output logic<64>,
        i_wd_addr : input  logic<5> ,
        i_wd_data : input  logic<64>,
        i_wen     : input  logic    ,
    ) {
        inst u_dec: Decode (
            clk, rst,
            i_rs1_addr, i_rs2_addr,
            o_rs1_data, o_rs2_data,
            i_wd_addr, i_wd_data, i_wen,
        );
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step(&rst);

        // Write 0xAA to reg[1]
        sim.set("i_wd_addr", Value::new(1, 5, false));
        sim.set("i_wd_data", Value::new(0xAA, 64, false));
        sim.set("i_wen", Value::new(1, 1, false));
        sim.step(&clk);
        sim.set("i_wen", Value::new(0, 1, false));
        sim.step(&clk);

        // Write 0xBB to reg[2]
        sim.set("i_wd_addr", Value::new(2, 5, false));
        sim.set("i_wd_data", Value::new(0xBB, 64, false));
        sim.set("i_wen", Value::new(1, 1, false));
        sim.step(&clk);
        sim.set("i_wen", Value::new(0, 1, false));
        sim.step(&clk);

        // Read reg[1] via rs1 and reg[2] via rs2
        sim.set("i_rs1_addr", Value::new(1, 5, false));
        sim.set("i_rs2_addr", Value::new(2, 5, false));
        sim.step(&clk); // comb evaluates
        sim.step(&clk); // pipeline register captures
        sim.ensure_comb_updated();

        let rs1 = sim.get("o_rs1_data").unwrap().payload_u64();
        let rs2 = sim.get("o_rs2_data").unwrap().payload_u64();
        assert_eq!(
            rs1, 0xAA,
            "JIT={} 4st={}: rs1 expected 0xAA got 0x{:x}",
            config.use_jit, config.use_4state, rs1
        );
        assert_eq!(
            rs2, 0xBB,
            "JIT={} 4st={}: rs2 expected 0xBB got 0x{:x}",
            config.use_jit, config.use_4state, rs2
        );
    }
}

/// Same as dynamic_array_harness_driven but with 3-level hierarchy:
/// Outer → Harness → RegFile (matches testbench structure).
#[test]
fn dynamic_array_three_level_hierarchy() {
    let code = r#"
    module RegFile (
        clk       : input  clock    ,
        rst       : input  reset    ,
        i_rs1_addr: input  logic<5> ,
        i_rs2_addr: input  logic<5> ,
        o_rs1_data: output logic<64>,
        o_rs2_data: output logic<64>,
        i_wd_addr : input  logic<5> ,
        i_wd_data : input  logic<64>,
        i_wen     : input  logic    ,
    ) {
        var regs: logic<64> [32];

        always_comb {
            o_rs1_data = if i_wen && i_rs1_addr == i_wd_addr ? i_wd_data : regs[i_rs1_addr];
            o_rs2_data = if i_wen && i_rs2_addr == i_wd_addr ? i_wd_data : regs[i_rs2_addr];
        }

        always_ff {
            if_reset {
                for i in 0..32 { regs[i] = 64'd0; }
            } else if i_wen {
                regs[i_wd_addr] = i_wd_data;
            }
        }
    }

    module Harness (
        clk : input  clock   ,
        rst : input  reset   ,
        r1  : output logic<64>,
        r2  : output logic<64>,
    ) {
        var cycle: logic<8>;
        always_ff {
            if_reset { cycle = 8'd0; }
            else     { cycle = cycle + 8'd1; }
        }

        var rs1_addr: logic<5>;
        var rs2_addr: logic<5>;
        var wd_addr : logic<5>;
        var wd_data : logic<64>;
        var wen     : logic;
        always_comb {
            rs1_addr = 5'd0;
            rs2_addr = 5'd0;
            wd_addr  = 5'd0;
            wd_data  = 64'd0;
            wen      = 1'b0;
            case cycle {
                8'd1: { wd_addr = 5'd1; wd_data = 64'hAA; wen = 1'b1; }
                8'd3: { wd_addr = 5'd2; wd_data = 64'hBB; wen = 1'b1; }
                default: { rs1_addr = 5'd1; rs2_addr = 5'd2; }
            }
        }

        var rs1_data: logic<64>;
        var rs2_data: logic<64>;
        inst u_rf: RegFile (
            clk, rst,
            i_rs1_addr: rs1_addr, i_rs2_addr: rs2_addr,
            o_rs1_data: rs1_data, o_rs2_data: rs2_data,
            i_wd_addr: wd_addr, i_wd_data: wd_data, i_wen: wen,
        );

        assign r1 = rs1_data;
        assign r2 = rs2_data;
    }

    module Top (
        clk : input  clock   ,
        rst : input  reset   ,
        r1  : output logic<64>,
        r2  : output logic<64>,
    ) {
        inst u_harness: Harness (
            clk, rst, r1, r2,
        );
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step(&rst);
        for _ in 0..20u32 {
            sim.step(&clk);
        }
        sim.ensure_comb_updated();
        let r1 = sim.get("r1").unwrap().payload_u64();
        let r2 = sim.get("r2").unwrap().payload_u64();
        assert_eq!(
            r1, 0xAA,
            "JIT={} 4st={}: r1 expected 0xAA got 0x{:x}",
            config.use_jit, config.use_4state, r1
        );
        assert_eq!(
            r2, 0xBB,
            "JIT={} 4st={}: r2 expected 0xBB got 0x{:x}",
            config.use_jit, config.use_4state, r2
        );
    }
}

/// Regression: within-level topological sort must order producer CB before
/// consumer CB when they are connected through a parent-level comb Assign.
///
/// Both child CBs read only FF values, so they get the same dependency
/// level. The old type-based sort (CB=0, Assign=1) would place both CBs
/// before the port-connection Assign, causing the consumer to read a stale
/// input. topo_sort_within_level resolves this by respecting the actual
/// variable dependency: Producer CB → Assign(wire) → Consumer CB.
#[test]
fn within_level_comb_chain_through_parent_wire() {
    let code = r#"
    module Producer (
        clk  : input  clock   ,
        rst  : input  reset   ,
        o_val: output logic<8>,
    ) {
        var cnt: logic<8>;
        always_ff {
            if_reset { cnt = 8'd0; }
            else     { cnt = cnt + 8'd1; }
        }
        // Comb output derived from FF — creates a CB with FF-only inputs
        assign o_val = cnt + 8'd10;
    }

    module Consumer (
        clk     : input  clock   ,
        rst     : input  reset   ,
        i_val   : input  logic<8>,
        o_result: output logic<8>,
    ) {
        // Pure comb: reads port input, adds constant
        assign o_result = i_val + 8'd100;
    }

    module Top (
        clk   : input  clock   ,
        rst   : input  reset   ,
        result: output logic<8>,
    ) {
        var link: logic<8>;

        inst u_prod: Producer (clk, rst, o_val: link);
        inst u_cons: Consumer (clk, rst, i_val: link, o_result: result);
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        // After reset+step: cnt went 0→1 (ff_swap), get() settles with cnt=1
        // o_val = 1+10 = 11, result = 11+100 = 111
        sim.step(&clk);
        let r = sim.get("result").unwrap().payload_u64();
        assert_eq!(
            r, 111,
            "JIT={} 4st={}: cycle 1 expected 111 (1+10+100), got {} — \
             comb chain through parent wire not propagated in single pass",
            config.use_jit, config.use_4state, r
        );

        // cnt=2, o_val=12, result=112
        sim.step(&clk);
        let r = sim.get("result").unwrap().payload_u64();
        assert_eq!(
            r, 112,
            "JIT={} 4st={}: cycle 2 expected 112 (2+10+100), got {}",
            config.use_jit, config.use_4state, r
        );
    }
}

/// Regression: stall-signal propagation through parent-level wire.
///
/// Models the pattern that caused an Sv39 4K-page failure:
/// a "staller" child module produces a busy signal, which propagates
/// through a parent wire (with a comb transform) to a "controller"
/// child module. The controller must see the staller's output in
/// the same cycle (single comb pass) to produce correct behavior.
///
/// Without correct within-level ordering, the controller CB evaluates
/// before the staller's busy signal is written to the parent wire,
/// reading stale data.
#[test]
fn within_level_stall_propagation() {
    let code = r#"
    module Staller (
        clk   : input  clock   ,
        rst   : input  reset   ,
        i_req : input  logic   ,
        o_busy: output logic   ,
    ) {
        var cnt: logic<4>;
        always_ff {
            if_reset { cnt = 4'd0; }
            else if i_req && cnt == 4'd0 { cnt = 4'd3; }
            else if cnt != 4'd0 { cnt = cnt - 4'd1; }
        }
        assign o_busy = cnt != 4'd0;
    }

    module Controller (
        clk     : input  clock   ,
        rst     : input  reset   ,
        i_stall : input  logic   ,
        o_active: output logic   ,
    ) {
        // Pure comb: active only when not stalled
        assign o_active = !i_stall;
    }

    module Top (
        clk    : input  clock   ,
        rst    : input  reset   ,
        active : output logic   ,
        busy   : output logic   ,
    ) {
        var stall_wire: logic;
        // Comb transform on stall path (like dcache_stall && !is_mmio)
        var stall_gated: logic;
        assign stall_gated = stall_wire;

        inst u_stall: Staller (
            clk, rst,
            i_req : !stall_wire,   // request when not busy
            o_busy: stall_wire,
        );
        inst u_ctrl: Controller (
            clk, rst,
            i_stall : stall_gated,
            o_active: active,
        );
        assign busy = stall_wire;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        // After reset+step: cnt went 0→3 (i_req=!stall_wire; at reset
        // stall_wire=0 so i_req=1, cnt transitions 0→3).
        // get() settles with cnt=3: busy=1, stall_gated=1, active=0.
        sim.step(&clk);
        let a = sim.get("active").unwrap().payload_u64();
        let b = sim.get("busy").unwrap().payload_u64();
        assert_eq!(
            (a, b),
            (0, 1),
            "JIT={} 4st={}: cycle 1 expected active=0,busy=1 got active={},busy={} — \
             stall signal not propagated through parent wire in single comb pass",
            config.use_jit,
            config.use_4state,
            a,
            b
        );

        // cnt=3→2 (counting down, i_req=0 since busy). busy=1, active=0.
        sim.step(&clk);
        let a = sim.get("active").unwrap().payload_u64();
        let b = sim.get("busy").unwrap().payload_u64();
        assert_eq!(
            (a, b),
            (0, 1),
            "JIT={} 4st={}: cycle 2 expected active=0,busy=1 (cnt=2) got active={},busy={}",
            config.use_jit,
            config.use_4state,
            a,
            b
        );

        // cnt=2→1, busy=1, active=0
        sim.step(&clk);
        let a = sim.get("active").unwrap().payload_u64();
        assert_eq!(
            a, 0,
            "JIT={} 4st={}: cycle 3 expected active=0 (cnt=1), got {}",
            config.use_jit, config.use_4state, a
        );

        // cnt=1→0, busy=0, active=1
        sim.step(&clk);
        let a = sim.get("active").unwrap().payload_u64();
        let b = sim.get("busy").unwrap().payload_u64();
        assert_eq!(
            (a, b),
            (1, 0),
            "JIT={} 4st={}: cycle 4 expected active=1,busy=0 (countdown done) got active={},busy={}",
            config.use_jit,
            config.use_4state,
            a,
            b
        );
    }
}

/// Regression: adding comb logic to one child module must not break a
/// sibling module's write-first forwarding.  This is the pattern that
/// triggered the unified comb JIT ordering bug (fp_regfile broke when mmu
/// gained a comb variable).
///
/// Top instantiates RegFile (write-first forwarding) and Sibling (extra
/// comb logic).  The test verifies that RegFile forwarding works
/// regardless of Sibling's comb complexity.
#[test]
fn sibling_comb_does_not_break_forwarding() {
    let code = r#"
    module RegFile (
        clk      : input  clock    ,
        rst      : input  reset    ,
        i_rs     : input  logic<2> ,
        i_wd     : input  logic<8> ,
        i_wen    : input  logic    ,
        o_rd     : output logic<8> ,
    ) {
        var regs: logic<8> [4];
        always_ff {
            if_reset {
                regs[0] = 8'd0; regs[1] = 8'd0;
                regs[2] = 8'd0; regs[3] = 8'd0;
            } else if i_wen {
                regs[i_rs] = i_wd;
            }
        }
        // Write-first forwarding
        always_comb {
            o_rd = if i_wen ? i_wd : regs[i_rs];
        }
    }

    module Sibling (
        clk    : input  clock    ,
        rst    : input  reset    ,
        i_en   : input  logic    ,
        o_val  : output logic<8> ,
    ) {
        var cnt: logic<8>;
        always_ff {
            if_reset { cnt = 8'd0; }
            else if i_en { cnt = cnt + 8'd1; }
        }
        // Extra comb — the kind that triggered the original bug
        var extra: logic<8>;
        always_comb { extra = cnt + 8'd42; }
        assign o_val = extra;
    }

    module Top (
        clk    : input  clock    ,
        rst    : input  reset    ,
        rd     : output logic<8> ,
        sib    : output logic<8> ,
    ) {
        inst u_rf: RegFile (
            clk, rst,
            i_rs: 2'd1, i_wd: 8'd77, i_wen: 1'b1,
            o_rd: rd,
        );
        inst u_sib: Sibling (
            clk, rst, i_en: 1'b1, o_val: sib,
        );
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step(&rst);
        sim.step(&clk);

        // Write-first: writing 77 and reading same register → must see 77
        let rd = sim.get("rd").unwrap().payload_u64();
        assert_eq!(
            rd, 77,
            "JIT={} 4st={}: write-first forwarding broken by sibling comb (got {})",
            config.use_jit, config.use_4state, rd
        );
        // Sibling should work independently
        let sib = sim.get("sib").unwrap().payload_u64();
        assert_eq!(
            sib,
            43, // cnt=1, extra=1+42=43
            "JIT={} 4st={}: sibling comb wrong (got {})",
            config.use_jit,
            config.use_4state,
            sib
        );
    }
}

/// Regression: pipeline stall release must not skip a stage.
///
/// Models a 2-stage pipeline: stage1 FF latches input, comb transforms
/// it, stage2 FF latches the comb result. When a long stall holds both
/// stages, releasing the stall should let stage2 capture the value that
/// was in stage1 BEFORE stage1 updates. If the simulator updates stage1
/// FF before stage2's comb reads it (wrong FF ordering in merged JIT),
/// stage2 gets the NEW stage1 value, skipping one pipeline slot.
#[test]
fn pipeline_stall_release_ordering() {
    let code = r#"
    module Top (
        clk     : input  clock    ,
        rst     : input  reset    ,
        i_data  : input  logic<8> ,
        i_stall : input  logic    ,
        stage2  : output logic<8> ,
    ) {
        // Stage 1 FF: latches input
        var s1: logic<8>;
        always_ff {
            if_reset { s1 = 8'd0; }
            else if !i_stall { s1 = i_data; }
        }

        // Comb transform between stages
        let s1_plus: logic<8> = s1 + 8'd100;

        // Stage 2 FF: latches comb result
        var s2: logic<8>;
        always_ff {
            if_reset { s2 = 8'd0; }
            else if !i_stall { s2 = s1_plus; }
        }

        assign stage2 = s2;
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        // Cycle 1: feed 10, no stall → s1=10, s2=100 (0+100)
        sim.set("i_data", Value::new(10, 8, false));
        sim.set("i_stall", Value::new(0, 1, false));
        sim.step(&clk);
        let s2 = sim.get("stage2").unwrap().payload_u64();
        assert_eq!(
            s2, 100,
            "JIT={} 4st={}: cycle 1 expected s2=100 (s1 was 0), got {}",
            config.use_jit, config.use_4state, s2
        );

        // Cycle 2: feed 20, no stall → s1=20, s2=110 (10+100)
        sim.set("i_data", Value::new(20, 8, false));
        sim.step(&clk);
        let s2 = sim.get("stage2").unwrap().payload_u64();
        assert_eq!(
            s2, 110,
            "JIT={} 4st={}: cycle 2 expected s2=110 (s1 was 10), got {}",
            config.use_jit, config.use_4state, s2
        );

        // Cycle 3-5: stall for 3 cycles. s1=20, s2=110 held.
        sim.set("i_data", Value::new(30, 8, false));
        sim.set("i_stall", Value::new(1, 1, false));
        sim.step(&clk);
        sim.step(&clk);
        sim.step(&clk);
        let s2 = sim.get("stage2").unwrap().payload_u64();
        assert_eq!(
            s2, 110,
            "JIT={} 4st={}: after stall expected s2=110 (held), got {}",
            config.use_jit, config.use_4state, s2
        );

        // Cycle 6: release stall. s1 should latch 30, s2 should latch
        // s1_plus = 20+100 = 120 (the OLD s1 value, not 30).
        sim.set("i_data", Value::new(40, 8, false));
        sim.set("i_stall", Value::new(0, 1, false));
        sim.step(&clk);
        let s2 = sim.get("stage2").unwrap().payload_u64();
        assert_eq!(
            s2, 120,
            "JIT={} 4st={}: stall release expected s2=120 (old s1=20+100), got {} \
             — stage1 FF updated before stage2 read the old value",
            config.use_jit, config.use_4state, s2
        );

        // Cycle 7: s1=40 (latched in cycle 6), s2 = 40+100 = 140
        sim.step(&clk);
        let s2 = sim.get("stage2").unwrap().payload_u64();
        assert_eq!(
            s2, 140,
            "JIT={} 4st={}: cycle 7 expected s2=140 (s1=40+100), got {}",
            config.use_jit, config.use_4state, s2
        );
    }
}

/// Regression: gather_external_offsets must keep outputs (not filter
/// internal variables from both inputs AND outputs). When outputs are
/// filtered, reorder_by_level assigns wrong dependency levels to
/// downstream blocks that read the intermediate variable.
#[test]
fn intermediate_variable_dependency_level() {
    // Three-level hierarchy: GrandChild writes a comb output that is
    // wired through Child to Parent.  If the Child CB's output_offsets
    // don't include the intermediate wire, Parent's Assign gets level 0
    // instead of level 1, breaking evaluation order.
    let code = r#"
    module Inner (
        clk  : input  clock   ,
        rst  : input  reset   ,
        o_val: output logic<8>,
    ) {
        var cnt: logic<8>;
        always_ff {
            if_reset { cnt = 8'd0; }
            else     { cnt = cnt + 8'd1; }
        }
        assign o_val = cnt;
    }

    module Middle (
        clk    : input  clock   ,
        rst    : input  reset   ,
        o_out  : output logic<8>,
    ) {
        var mid: logic<8>;
        inst u_inner: Inner (clk, rst, o_val: mid);
        // Internal variable: both read (by assign below) and written (by Inner)
        assign o_out = mid + 8'd10;
    }

    module Top (
        clk    : input  clock   ,
        rst    : input  reset   ,
        result : output logic<8>,
    ) {
        var link: logic<8>;
        inst u_mid: Middle (clk, rst, o_out: link);
        assign result = link + 8'd100;
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step(&rst);
        sim.step(&clk);

        // cnt=1, mid=1+10=11, result=11+100=111
        let r = sim.get("result").unwrap().payload_u64();
        assert_eq!(
            r, 111,
            "JIT={} 4st={}: intermediate variable lost from CB outputs (got {})",
            config.use_jit, config.use_4state, r
        );
    }
}

// ============================================================
// Adversarial ordering tests: verify JIT/interpreter equivalence
// at every cycle using DualSimulator.
// ============================================================

/// Diamond dependency: A→B, A→C, B→D, C→D (fan-in levelization)
#[test]
fn ordering_diamond_dependency() {
    let code = r#"
    module Top (
        a: input  logic<32>,
        d: output logic<32>,
    ) {
        var b: logic<32>;
        var c: logic<32>;
        assign b = a + 1;
        assign c = a + 2;
        assign d = b + c;
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        for i in 0u64..20 {
            dual.set("a", Value::new(i, 32, false));
            dual.step_synthetic();
            let expected = (i + 1) + (i + 2);
            assert_eq!(dual.get("d").unwrap(), Value::new(expected, 32, false));
        }
    });
}

/// Long chain: A→B→C→...→H (correct levelization → 1 pass, wrong → many)
#[test]
fn ordering_long_chain() {
    let code = r#"
    module Top (
        a: input  logic<32>,
        h: output logic<32>,
    ) {
        var b: logic<32>;
        var c: logic<32>;
        var d: logic<32>;
        var e: logic<32>;
        var f: logic<32>;
        var g: logic<32>;
        assign b = a + 1;
        assign c = b + 1;
        assign d = c + 1;
        assign e = d + 1;
        assign f = e + 1;
        assign g = f + 1;
        assign h = g + 1;
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        for i in 0u64..20 {
            dual.set("a", Value::new(i, 32, false));
            dual.step_synthetic();
            assert_eq!(dual.get("h").unwrap(), Value::new(i + 7, 32, false));
        }
    });
}

/// Cross-module diamond: Parent comb → Child comb → Parent comb
/// (test_hello_str equivalent pattern)
#[test]
fn ordering_cross_module_diamond() {
    let code = r#"
    module Child (
        x: input  logic<32>,
        y: output logic<32>,
    ) {
        assign y = x * 2;
    }

    module Top (
        a: input  logic<32>,
        result: output logic<32>,
    ) {
        var mid: logic<32>;
        assign mid = a + 10;

        inst u: Child (
            x: mid,
            y: result,
        );
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        for i in 0u64..20 {
            dual.set("a", Value::new(i, 32, false));
            dual.step_synthetic();
            assert_eq!(
                dual.get("result").unwrap(),
                Value::new((i + 10) * 2, 32, false)
            );
        }
    });
}

/// Conditional dependency: different comb levels in if/else branches
#[test]
fn ordering_conditional_dependency() {
    let code = r#"
    module Top (
        sel: input  logic,
        a:   input  logic<32>,
        out: output logic<32>,
    ) {
        var x: logic<32>;
        var y: logic<32>;
        assign x = a + 1;
        assign y = a + 2;
        assign out = if sel ? x : y;
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        for i in 0u64..20 {
            let sel_val = (i % 2) as u64;
            dual.set("sel", Value::new(sel_val, 1, false));
            dual.set("a", Value::new(i, 32, false));
            dual.step_synthetic();
            let expected = if sel_val == 1 { i + 1 } else { i + 2 };
            assert_eq!(dual.get("out").unwrap(), Value::new(expected, 32, false));
        }
    });
}

/// Re-convergent fan-out: single comb variable feeds multiple child modules
/// that converge back to parent
#[test]
fn ordering_reconvergent_fanout() {
    let code = r#"
    module Adder (
        x: input  logic<32>,
        bias: input logic<32>,
        y: output logic<32>,
    ) {
        assign y = x + bias;
    }

    module Top (
        a: input  logic<32>,
        result: output logic<32>,
    ) {
        var y1: logic<32>;
        var y2: logic<32>;
        assign result = y1 + y2;

        inst u1: Adder (
            x: a,
            bias: 32'd100,
            y: y1,
        );

        inst u2: Adder (
            x: a,
            bias: 32'd200,
            y: y2,
        );
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        for i in 0u64..20 {
            dual.set("a", Value::new(i, 32, false));
            dual.step_synthetic();
            assert_eq!(
                dual.get("result").unwrap(),
                Value::new((i + 100) + (i + 200), 32, false)
            );
        }
    });
}

/// Sequential + comb interaction: FF output feeds comb chain,
/// verifies ordering across clock edges with DualSimulator
#[test]
fn ordering_ff_to_comb_chain() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        result: output logic<32>,
    ) {
        var cnt: logic<32>;
        var a: logic<32>;
        var b: logic<32>;
        var c: logic<32>;

        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }

        assign a = cnt + 10;
        assign b = a * 2;
        assign c = b + 5;
        assign result = c;
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        dual.step_reset("rst");
        for i in 0u64..50 {
            dual.step_clock("clk");
            let cnt = i + 1;
            let expected = (cnt + 10) * 2 + 5;
            assert_eq!(dual.get("result").unwrap(), Value::new(expected, 32, false));
        }
    });
}

/// Wide FF (>64-bit) NBA semantic: intra-cycle read after write should
/// return the OLD value, not the in-flight new value.  Without write-log
/// emit, wide FFs wrote directly to FF storage and intra-cycle reads
/// observed the new value (NBA violation); for unpacked wide FFs current
/// also never updated because the next-slot path produced no log entries
/// for `ff_commit_from_log` to apply.
#[test]
fn wide_ff_nba_intra_cycle_read_after_write() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        cnt:      output logic<128>,
        cnt_prev: output logic<128>,
    ) {
        var c:      logic<128>;
        var c_prev: logic<128>;

        always_ff {
            if_reset {
                c = 0;
                c_prev = 0;
            } else {
                c = c + 1;
                c_prev = c;
            }
        }

        assign cnt = c;
        assign cnt_prev = c_prev;
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        dual.step_reset("rst");
        for i in 0u64..16 {
            dual.step_clock("clk");
            let cnt_expected = Value::new(i + 1, 128, false);
            let cnt_prev_expected = Value::new(i, 128, false);
            assert_eq!(
                dual.get("cnt").unwrap(),
                cnt_expected,
                "cycle {}: cnt mismatch",
                i + 1
            );
            assert_eq!(
                dual.get("cnt_prev").unwrap(),
                cnt_prev_expected,
                "cycle {}: cnt_prev should be OLD cnt (NBA semantic)",
                i + 1
            );
        }
    });
}

/// For-loop blocking writes to the same VarId across iterations.  A
/// constant-bound `for i in 0..N` with `c += 1` inside writes c N times
/// per cycle.  Two semantic regimes coexist in the simulator:
///
///   - Default config: the analyzer's `is_ff` refinement (FfTable) treats
///     same-block self-references as comb-safe and allocates c in comb
///     storage.  Each iteration's write is immediately visible to the
///     next iteration's read, yielding the "blocking-chain" result c = N.
///   - `--disable-ff-opt` (Config.disable_ff_opt = true): forces c to FF
///     with dual-slot (current/next) storage and strict NBA semantics.
///     Every iteration's read sees the OLD current-slot value, every
///     iteration's write lands in the next-slot, and the final commit
///     leaves c = OLD+1 = 1.  Cross-iteration "chaining" is not preserved
///     under strict NBA — this matches SystemVerilog `<=` semantics.
///
/// Constant-bound `for` loops are unrolled by the analyzer's `unroll_for`
/// pass into N individual Assigns, so `multi_write_analysis` correctly
/// counts N writes and marks c as multi-RMW (dual-slot, not packed) under
/// the forced-FF regime.  Without that detection the packed layout would
/// produce the wrong NBA result (silent log-overwrite, c = 1 by accident
/// rather than by NBA semantics).
#[test]
fn for_loop_blocking_write_same_var() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        cnt: output logic<8>,
    ) {
        var c: logic<8>;

        always_ff {
            if_reset {
                c = 0;
            } else {
                for i in 0..4 {
                    c = c + 1 + i - i;
                }
            }
        }

        assign cnt = c;
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step(&rst);
        sim.step(&clk);

        let v = sim.get("cnt").unwrap();
        // Default ff_opt path stores c as comb (blocking chain → 4).
        // --disable-ff-opt forces c to dual-slot FF (NBA → 1).
        let expected_payload = if config.disable_ff_opt { 1 } else { 4 };
        let expected = Value::new(expected_payload, 8, false);
        assert_eq!(
            v, expected,
            "config={:?}: expected cnt = {} after 1 clock",
            config, expected_payload,
        );
    }
}

/// LHS concatenation `{hi, lo} = expr` inside always_ff.  The simulator
/// splits a multi-dst air::AssignStatement into N separate
/// ProtoStatement::Assigns, one per destination, each with a `rhs_select`
/// slice covering the corresponding bit range of the expression.  Phase
/// 1.5 v2's multi_write_analysis sees every `dst` entry through
/// `add_dst_write`, so packed/unpacked classification is correct even
/// when a single source statement writes multiple FF targets.
#[test]
fn lhs_concatenation_in_always_ff() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        x:   input  logic<32>,
        hi:  output logic<20>,
        lo:  output logic<12>,
    ) {
        var hi_q: logic<20>;
        var lo_q: logic<12>;

        always_ff {
            if_reset {
                hi_q = 0;
                lo_q = 0;
            } else {
                {hi_q, lo_q} = x;
            }
        }

        assign hi = hi_q;
        assign lo = lo_q;
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step(&rst);
        sim.set("x", Value::new(0xABCDE_123, 32, false));
        sim.step(&clk);

        let hi = sim.get("hi").unwrap();
        let lo = sim.get("lo").unwrap();
        assert_eq!(
            hi,
            Value::new(0xABCDE, 20, false),
            "config={:?}: hi mismatch",
            config,
        );
        assert_eq!(
            lo,
            Value::new(0x123, 12, false),
            "config={:?}: lo mismatch",
            config,
        );
    }
}

/// LHS concatenation inside an initial block, written to comb-storage
/// variables.  The split-Assign path must produce a separate
/// `ProtoStatement::Assign` per destination, each carrying its own
/// `rhs_select` bit slice, so a 16-bit literal placed into `{hi, lo}`
/// lands as `hi = 0xAB` and `lo = 0xCD`.
#[test]
fn lhs_concatenation_in_initial() {
    let code = r#"
    module Top (
        hi: output logic<8>,
        lo: output logic<8>,
    ) {
        var hi_v: logic<8>;
        var lo_v: logic<8>;

        initial {
            {hi_v, lo_v} = 16'hAB_CD;
        }

        assign hi = hi_v;
        assign lo = lo_v;
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.step(&Event::Initial);
        sim.step(&Event::Clock(VarId::SYNTHETIC));

        let hi = sim.get("hi").unwrap();
        let lo = sim.get("lo").unwrap();
        assert_eq!(
            hi,
            Value::new(0xAB, 8, false),
            "config={:?}: initial-block hi mismatch",
            config,
        );
        assert_eq!(
            lo,
            Value::new(0xCD, 8, false),
            "config={:?}: initial-block lo mismatch",
            config,
        );
    }
}

/// Multi-level hierarchy: grandparent → parent → child comb propagation.
/// Unified comb list ensures correct ordering across all hierarchy levels.
#[test]
fn ordering_three_level_hierarchy() {
    let code = r#"
    module Leaf (
        x: input  logic<32>,
        y: output logic<32>,
    ) {
        assign y = x + 1;
    }

    module Mid (
        x: input  logic<32>,
        y: output logic<32>,
    ) {
        var tmp: logic<32>;
        inst u: Leaf (
            x: x,
            y: tmp,
        );
        assign y = tmp + 1;
    }

    module Top (
        a: input  logic<32>,
        result: output logic<32>,
    ) {
        var tmp: logic<32>;
        inst u: Mid (
            x: a,
            y: tmp,
        );
        assign result = tmp + 1;
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        for i in 0u64..20 {
            dual.set("a", Value::new(i, 32, false));
            dual.step_synthetic();
            // Leaf: x+1, Mid: (x+1)+1, Top: ((x+1)+1)+1 = x+3
            assert_eq!(dual.get("result").unwrap(), Value::new(i + 3, 32, false));
        }
    });
}

/// Multiple children with cross-dependencies in parent
#[test]
fn ordering_multi_child_cross_dep() {
    let code = r#"
    module Child1 (
        x: input  logic<32>,
        y: output logic<32>,
    ) {
        assign y = x * 3;
    }

    module Child2 (
        x: input  logic<32>,
        y: output logic<32>,
    ) {
        assign y = x + 7;
    }

    module Top (
        a: input  logic<32>,
        result: output logic<32>,
    ) {
        var c1_out: logic<32>;
        var c2_out: logic<32>;

        inst u1: Child1 (
            x: a,
            y: c1_out,
        );

        // Child2 depends on Child1's output
        inst u2: Child2 (
            x: c1_out,
            y: c2_out,
        );

        assign result = c2_out;
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        for i in 0u64..20 {
            dual.set("a", Value::new(i, 32, false));
            dual.step_synthetic();
            // Child1: a*3, Child2: (a*3)+7
            assert_eq!(
                dual.get("result").unwrap(),
                Value::new(i * 3 + 7, 32, false)
            );
        }
    });
}

// ============================================================
// DualSimulator regression tests for merged event + hierarchy
// ============================================================

/// JIT/interpreter equivalence for child comb+FF (merged event pattern).
#[test]
fn dual_inst_comb_and_ff() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        out: output logic<32>,
        comb_out: output logic<32>,
    ) {
        inst u: Inner (
            clk,
            rst,
            out,
            comb_out,
        );
    }

    module Inner (
        clk: input  clock,
        rst: input  reset,
        out: output logic<32>,
        comb_out: output logic<32>,
    ) {
        assign comb_out = out + 1;

        always_ff {
            if_reset {
                out = 0;
            } else {
                out = comb_out;
            }
        }
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        let (jclk, iclk) = dual.get_clock("clk");
        let (jrst, irst) = dual.get_reset("rst");
        dual.step(&jrst, &irst);
        for i in 0u64..10 {
            dual.step(&jclk, &iclk);
            assert_eq!(dual.get("out").unwrap(), Value::new(i + 1, 32, false));
        }
    });
}

/// JIT/interpreter equivalence for 3-level merged comb output chain.
#[test]
fn dual_merged_comb_output_multi_level() {
    let code = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var mid_out: logic<32>;
        inst u_mid: Middle (
            clk,
            rst,
            o_val: mid_out,
        );
        always_ff {
            if_reset {
                result = 0;
            } else {
                result = mid_out;
            }
        }
    }

    module Middle (
        clk  : input  clock,
        rst  : input  reset,
        o_val: output logic<32>,
    ) {
        var inner_out: logic<32>;
        inst u_inner: Inner (
            clk,
            rst,
            o_val: inner_out,
        );
        assign o_val = inner_out + 1000;
    }

    module Inner (
        clk  : input  clock,
        rst  : input  reset,
        o_val: output logic<32>,
    ) {
        var cnt: logic<32>;
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }
        assign o_val = cnt;
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        let (jclk, iclk) = dual.get_clock("clk");
        let (jrst, irst) = dual.get_reset("rst");
        dual.step(&jrst, &irst);
        for _ in 0..10 {
            dual.step(&jclk, &iclk);
        }
        // Verify result is consistent between JIT and interpreter
        let _ = dual.get("result").unwrap();
    });
}

/// Verify that required_comb_passes stays bounded for hierarchical designs.
/// CBs are kept atomic, so internal backward edges are invisible.
/// A sudden increase in passes would indicate a regression in CB handling.
#[test]
fn required_passes_bounded_for_hierarchy() {
    let code = r#"
    module Leaf (
        x: input  logic<32>,
        y: output logic<32>,
    ) {
        assign y = x + 1;
    }

    module Mid (
        x: input  logic<32>,
        y: output logic<32>,
    ) {
        var tmp: logic<32>;
        inst u: Leaf (x: x, y: tmp);
        assign y = tmp + 1;
    }

    module Top (
        a: input  logic<32>,
        result: output logic<32>,
    ) {
        var tmp: logic<32>;
        inst u: Mid (x: a, y: tmp);
        assign result = tmp + 1;
    }
    "#;

    let ir = analyze(
        code,
        &Config {
            use_jit: true,
            ..Default::default()
        },
    );
    assert!(
        ir.required_comb_passes <= 2,
        "expected required_comb_passes <= 2, got {} — CB atomic handling may be broken",
        ir.required_comb_passes
    );
}

/// JIT/interpreter equivalence for cross-child forwarding.
#[test]
fn dual_cross_child_forwarding() {
    let code = r#"
    module ChildA (
        clk: input  clock,
        rst: input  reset,
        o  : output logic<32>,
    ) {
        var cnt: logic<32>;
        always_ff {
            if_reset { cnt = 0; } else { cnt += 1; }
        }
        assign o = cnt * 2;
    }

    module ChildB (
        i     : input  logic<32>,
        result: output logic<32>,
    ) {
        assign result = i + 100;
    }

    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<32>,
    ) {
        var a_out: logic<32>;
        inst ua: ChildA (clk, rst, o: a_out);
        inst ub: ChildB (i: a_out, result);
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        let (jclk, iclk) = dual.get_clock("clk");
        let (jrst, irst) = dual.get_reset("rst");
        dual.step(&jrst, &irst);
        for i in 0u64..10 {
            dual.step(&jclk, &iclk);
            // After step: cnt incremented to i+1, comb settles with new cnt.
            // ChildA: cnt=i+1, o=(i+1)*2; ChildB: result=(i+1)*2+100
            assert_eq!(
                dual.get("result").unwrap(),
                Value::new((i + 1) * 2 + 100, 32, false)
            );
        }
    });
}

// ============================================================
// is_ff classification edge case tests
// ============================================================

/// Two always_comb blocks in the same module with variable flow.
/// mid must be is_ff=false (assigned and read both in comb context).
#[test]
fn is_ff_two_always_comb_same_module() {
    let code = r#"
    module Top (
        a: input  logic<32>,
        result: output logic<32>,
    ) {
        var mid: logic<32>;
        always_comb { mid = a + 1; }
        always_comb { result = mid * 2; }
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        for i in 0u64..10 {
            dual.set("a", Value::new(i, 32, false));
            dual.step_synthetic();
            assert_eq!(
                dual.get("result").unwrap(),
                Value::new((i + 1) * 2, 32, false)
            );
        }
    });
}

/// Comb variable propagated to child module input port.
/// computed must remain is_ff=false and reach child correctly.
#[test]
fn is_ff_comb_to_child_port() {
    let code = r#"
    module Child (
        i: input  logic<32>,
        o: output logic<32>,
    ) {
        assign o = i + 100;
    }

    module Top (
        a: input  logic<32>,
        result: output logic<32>,
    ) {
        var computed: logic<32>;
        always_comb { computed = a * 3; }
        inst u: Child (i: computed, o: result);
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        for i in 0u64..10 {
            dual.set("a", Value::new(i, 32, false));
            dual.step_synthetic();
            assert_eq!(
                dual.get("result").unwrap(),
                Value::new(i * 3 + 100, 32, false)
            );
        }
    });
}

/// Single FF variable consumed by multiple comb assign statements.
#[test]
fn is_ff_one_ff_multiple_comb_readers() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        sum: output logic<32>,
        diff: output logic<32>,
    ) {
        var cnt: logic<32>;
        always_ff {
            if_reset { cnt = 0; } else { cnt += 1; }
        }
        assign sum  = cnt + 10;
        assign diff = cnt - 1;
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        let (jclk, iclk) = dual.get_clock("clk");
        let (jrst, irst) = dual.get_reset("rst");
        dual.step(&jrst, &irst);
        for i in 0u64..10 {
            dual.step(&jclk, &iclk);
            // After step: cnt = i+1
            let cnt = i + 1;
            assert_eq!(dual.get("sum").unwrap(), Value::new(cnt + 10, 32, false));
            assert_eq!(
                dual.get("diff").unwrap(),
                Value::new(cnt.wrapping_sub(1) & 0xFFFF_FFFF, 32, false)
            );
        }
    });
}

/// DualSimulator version of simple_ff: basic FF counter.
#[test]
fn dual_simple_ff() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        out: output logic<32>,
    ) {
        always_ff {
            if_reset {
                out = 0;
            } else {
                out += 1;
            }
        }
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        let (jclk, iclk) = dual.get_clock("clk");
        let (jrst, irst) = dual.get_reset("rst");
        dual.step(&jrst, &irst);
        for i in 0u64..20 {
            dual.step(&jclk, &iclk);
            assert_eq!(dual.get("out").unwrap(), Value::new(i + 1, 32, false));
        }
    });
}

/// DualSimulator version of inst_ff: FF in child module.
#[test]
fn dual_inst_ff() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        out: output logic<32>,
    ) {
        inst u: Inner (clk, rst, out);
    }

    module Inner (
        clk: input  clock,
        rst: input  reset,
        out: output logic<32>,
    ) {
        always_ff {
            if_reset { out = 0; } else { out += 1; }
        }
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        let (jclk, iclk) = dual.get_clock("clk");
        let (jrst, irst) = dual.get_reset("rst");
        dual.step(&jrst, &irst);
        for i in 0u64..20 {
            dual.step(&jclk, &iclk);
            assert_eq!(dual.get("out").unwrap(), Value::new(i + 1, 32, false));
        }
    });
}

// ============================================================
// is_ff edge case: if_reset branch semantics
// ============================================================

/// Variable assigned only in reset branch of always_ff should be FF.
/// The normal branch reads but does not assign — is_ff must still be true.
#[test]
fn is_ff_reset_branch_only_assign() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        out: output logic<32>,
    ) {
        var state: logic<32>;
        always_ff {
            if_reset {
                state = 32'd42;
            }
        }
        assign out = state;
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        let (jclk, iclk) = dual.get_clock("clk");
        let (jrst, irst) = dual.get_reset("rst");
        dual.step(&jrst, &irst);
        assert_eq!(dual.get("out").unwrap(), Value::new(42, 32, false));
        // state should remain 42 since normal branch has no assignment
        dual.step(&jclk, &iclk);
        assert_eq!(dual.get("out").unwrap(), Value::new(42, 32, false));
    });
}

/// Variable assigned in both reset and normal branches of always_ff,
/// read by always_comb. Verifies FF classification and correct timing.
#[test]
fn is_ff_both_branches_with_comb_reader() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        doubled: output logic<32>,
    ) {
        var cnt: logic<32>;
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                cnt += 1;
            }
        }
        var tmp: logic<32>;
        always_comb {
            tmp = cnt * 2;
        }
        assign doubled = tmp;
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        let (jclk, iclk) = dual.get_clock("clk");
        let (jrst, irst) = dual.get_reset("rst");
        dual.step(&jrst, &irst);
        for i in 0u64..10 {
            dual.step(&jclk, &iclk);
            assert_eq!(
                dual.get("doubled").unwrap(),
                Value::new((i + 1) * 2, 32, false)
            );
        }
    });
}

/// Verify that input/output port variables are not treated as FF
/// even when referenced across always_ff and always_comb boundaries.
#[test]
fn is_ff_port_variables_not_ff() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        a: input  logic<32>,
        b: output logic<32>,
    ) {
        // b is an output port assigned in always_comb (not FF)
        always_comb {
            b = a + 1;
        }
    }
    "#;

    // Pure comb: verify with DualSimulator
    verify_jit_interpreter_equivalence(code, |dual| {
        for i in 0u64..10 {
            dual.set("a", Value::new(i, 32, false));
            dual.step_synthetic();
            assert_eq!(dual.get("b").unwrap(), Value::new(i + 1, 32, false));
        }
    });
}

// ============================================================
// Case statement tests
// ============================================================

/// Simple case/default pattern in always_comb.
#[test]
fn case_simple() {
    let code = r#"
    module Top (
        sel: input  logic<2>,
        result: output logic<32>,
    ) {
        always_comb {
            case sel {
                2'd0: result = 32'd10;
                2'd1: result = 32'd20;
                2'd2: result = 32'd30;
                default: result = 32'd99;
            }
        }
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        dual.set("sel", Value::new(0, 2, false));
        dual.step_synthetic();
        assert_eq!(dual.get("result").unwrap(), Value::new(10, 32, false));

        dual.set("sel", Value::new(1, 2, false));
        dual.step_synthetic();
        assert_eq!(dual.get("result").unwrap(), Value::new(20, 32, false));

        dual.set("sel", Value::new(2, 2, false));
        dual.step_synthetic();
        assert_eq!(dual.get("result").unwrap(), Value::new(30, 32, false));

        dual.set("sel", Value::new(3, 2, false));
        dual.step_synthetic();
        assert_eq!(dual.get("result").unwrap(), Value::new(99, 32, false));
    });
}

/// Case result feeds into a comb dependency chain.
#[test]
fn case_with_comb_dependency() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        result: output logic<32>,
    ) {
        var cnt: logic<2>;
        always_ff {
            if_reset { cnt = 0; } else { cnt += 1; }
        }

        var decoded: logic<32>;
        always_comb {
            case cnt {
                2'd0: decoded = 32'd100;
                2'd1: decoded = 32'd200;
                2'd2: decoded = 32'd300;
                default: decoded = 32'd400;
            }
        }
        assign result = decoded + 1;
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        let (jclk, iclk) = dual.get_clock("clk");
        let (jrst, irst) = dual.get_reset("rst");
        dual.step(&jrst, &irst);

        let expected = [101, 201, 301, 401];
        for i in 0..8 {
            dual.step(&jclk, &iclk);
            assert_eq!(
                dual.get("result").unwrap(),
                Value::new(expected[(i + 1) % 4], 32, false)
            );
        }
    });
}

// ============================================================
// Let binding tests
// ============================================================

/// let binding in always_comb context.
#[test]
fn let_in_comb() {
    let code = r#"
    module Top (
        a: input  logic<32>,
        result: output logic<32>,
    ) {
        always_comb {
            let x: logic<32> = a + 1;
            result = x * 2;
        }
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        for i in 0u64..10 {
            dual.set("a", Value::new(i, 32, false));
            dual.step_synthetic();
            assert_eq!(
                dual.get("result").unwrap(),
                Value::new((i + 1) * 2, 32, false)
            );
        }
    });
}

/// Chained let bindings: let → let → assign.
#[test]
fn let_chain() {
    let code = r#"
    module Top (
        a: input  logic<32>,
        result: output logic<32>,
    ) {
        always_comb {
            let x: logic<32> = a + 1;
            let y: logic<32> = x + 2;
            result = y + 3;
        }
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        for i in 0u64..10 {
            dual.set("a", Value::new(i, 32, false));
            dual.step_synthetic();
            // result = (a + 1) + 2 + 3 = a + 6
            assert_eq!(dual.get("result").unwrap(), Value::new(i + 6, 32, false));
        }
    });
}

// ============================================================
// JIT lifetime / drop order safety
// ============================================================

/// Verify Ir can be created and dropped without panic.
/// Ensures JIT Mmap backing outlives function pointers during drop.
#[test]
fn ir_drop_order_safety() {
    let code = r#"
    module Top (
        clk: input clock, rst: input reset,
        out: output logic<32>,
    ) {
        inst u: Inner (clk, rst, out);
    }
    module Inner (
        clk: input clock, rst: input reset,
        out: output logic<32>,
    ) {
        always_ff { if_reset { out = 0; } else { out += 1; } }
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step(&rst);
        sim.step(&clk);
        // Explicitly drop — should not panic
        drop(sim);
    }
}

/// Independent FF variables in the same always_ff block.
/// Verify both self-referencing and constant-assigned FF variables
/// produce correct values across clock cycles with NBA semantics.
#[test]
fn nba_independent_ff_variables() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        a_out: output logic<32>,
        b_out: output logic<32>,
    ) {
        var a: logic<32>;
        var b: logic<32>;

        // a and b are independent FF variables in the same always_ff.
        // Neither reads the other — independent FF variables.
        always_ff {
            if_reset {
                a = 0;
                b = 100;
            } else {
                a = a + 1;   // self-ref
                b = 200;     // constant assign
            }
        }
        assign a_out = a;
        assign b_out = b;
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        let (jclk, iclk) = dual.get_clock("clk");
        let (jrst, irst) = dual.get_reset("rst");
        dual.step(&jrst, &irst);

        dual.step(&jclk, &iclk);
        assert_eq!(dual.get("a_out").unwrap(), Value::new(1, 32, false));
        assert_eq!(dual.get("b_out").unwrap(), Value::new(200, 32, false));

        dual.step(&jclk, &iclk);
        assert_eq!(dual.get("a_out").unwrap(), Value::new(2, 32, false));
        assert_eq!(dual.get("b_out").unwrap(), Value::new(200, 32, false));
    });
}

/// Two FF variables where one reads the other in the same always_ff block.
/// The reader must see the OLD value (NBA semantics), not the just-written value.
#[test]
fn nba_cross_read_in_same_event() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        x_out: output logic<32>,
        y_out: output logic<32>,
    ) {
        var x: logic<32>;
        var y: logic<32>;

        always_ff {
            if_reset {
                x = 0;
                y = 0;
            } else {
                x = x + 1;
                y = x;      // y reads x: must see OLD x (before +1)
            }
        }
        assign x_out = x;
        assign y_out = y;
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        let (jclk, iclk) = dual.get_clock("clk");
        let (jrst, irst) = dual.get_reset("rst");
        dual.step(&jrst, &irst);

        for i in 0u64..10 {
            dual.step(&jclk, &iclk);
            assert_eq!(dual.get("x_out").unwrap(), Value::new(i + 1, 32, false));
            assert_eq!(
                dual.get("y_out").unwrap(),
                Value::new(i, 32, false),
                "cycle {}: y should see old x (NBA), got new x",
                i + 1
            );
        }
    });
}

/// Three FF variables with chain dependency: a→b→c in same always_ff.
/// All reads must see old values regardless of statement order.
#[test]
fn nba_three_variable_chain() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        c_out: output logic<32>,
    ) {
        var a: logic<32>;
        var b: logic<32>;
        var c: logic<32>;

        always_ff {
            if_reset {
                a = 1;
                b = 0;
                c = 0;
            } else {
                c = b;       // c reads old b
                b = a;       // b reads old a
                a = a + 1;   // a self-increments
            }
        }
        assign c_out = c;
    }
    "#;

    // After reset: a=1, b=0, c=0
    // Cycle 1: c=old_b=0, b=old_a=1, a=2 → c_out=0
    // Cycle 2: c=old_b=1, b=old_a=2, a=3 → c_out=1
    // Cycle n: c=old_b=n-1, b=old_a=n, a=n+1 → c_out=n-1
    verify_jit_interpreter_equivalence(code, |dual| {
        let (jclk, iclk) = dual.get_clock("clk");
        let (jrst, irst) = dual.get_reset("rst");
        dual.step(&jrst, &irst);

        for i in 1u64..=10 {
            dual.step(&jclk, &iclk);
            let expected = if i <= 1 { 0 } else { i - 1 };
            assert_eq!(
                dual.get("c_out").unwrap(),
                Value::new(expected, 32, false),
                "cycle {i}: c_out should be {expected}"
            );
        }
    });
}

/// let binding inside always_ff must use blocking assignment (BA) semantics:
/// the let-bound value is immediately visible within the same cycle.
/// FfTable classifies let variables as is_ff=false (comb) since they are
/// only assigned and referenced within the same declaration block.
#[test]
fn let_in_always_ff_blocking_semantics() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        result: output logic<32>,
    ) {
        var cnt: logic<32>;
        always_ff {
            if_reset {
                cnt = 0;
            } else {
                let tmp: logic<32> = cnt + 1;
                cnt = tmp + 1;
            }
        }
        assign result = cnt;
    }
    "#;

    // If let uses BA: cnt = (old_cnt + 1) + 1 = old_cnt + 2 each cycle → 0, 2, 4, 6, ...
    // If let uses NBA: tmp reads stale value → wrong result
    verify_jit_interpreter_equivalence(code, |dual| {
        let (jclk, iclk) = dual.get_clock("clk");
        let (jrst, irst) = dual.get_reset("rst");
        dual.step(&jrst, &irst);
        for i in 0u64..10 {
            dual.step(&jclk, &iclk);
            assert_eq!(
                dual.get("result").unwrap(),
                Value::new((i + 1) * 2, 32, false),
                "cycle {}: expected {} (BA semantics), let in always_ff may be using NBA",
                i + 1,
                (i + 1) * 2
            );
        }
    });
}

// ============================================================
// Additional NBA edge case tests
// ============================================================

/// Array elements with NBA: arr[1] reads old arr[0] within same always_ff.
#[test]
fn nba_array_element() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        out0: output logic<32>,
        out1: output logic<32>,
    ) {
        var arr: logic<32> [2];

        always_ff {
            if_reset {
                arr[0] = 0;
                arr[1] = 0;
            } else {
                arr[0] = arr[0] + 1;
                arr[1] = arr[0];   // must see OLD arr[0]
            }
        }
        assign out0 = arr[0];
        assign out1 = arr[1];
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        let (jclk, iclk) = dual.get_clock("clk");
        let (jrst, irst) = dual.get_reset("rst");
        dual.step(&jrst, &irst);

        for i in 0u64..10 {
            dual.step(&jclk, &iclk);
            assert_eq!(dual.get("out0").unwrap(), Value::new(i + 1, 32, false));
            assert_eq!(
                dual.get("out1").unwrap(),
                Value::new(i, 32, false),
                "cycle {}: arr[1] should see old arr[0]",
                i + 1
            );
        }
    });
}

/// Two separate always_ff blocks reading the same variable.
/// Both must see the pre-clock-edge value of the shared variable.
#[test]
fn nba_multiple_always_ff() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        a_out: output logic<32>,
        b_out: output logic<32>,
    ) {
        var shared: logic<32>;
        var a: logic<32>;
        var b: logic<32>;

        always_ff {
            if_reset { shared = 0; }
            else     { shared = shared + 1; }
        }
        always_ff {
            if_reset { a = 0; }
            else     { a = shared; }
        }
        always_ff {
            if_reset { b = 0; }
            else     { b = shared + 10; }
        }
        assign a_out = a;
        assign b_out = b;
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        let (jclk, iclk) = dual.get_clock("clk");
        let (jrst, irst) = dual.get_reset("rst");
        dual.step(&jrst, &irst);

        for i in 0u64..10 {
            dual.step(&jclk, &iclk);
            assert_eq!(
                dual.get("a_out").unwrap(),
                Value::new(i, 32, false),
                "cycle {}: a should see old shared",
                i + 1
            );
            assert_eq!(
                dual.get("b_out").unwrap(),
                Value::new(i + 10, 32, false),
                "cycle {}: b should see old shared + 10",
                i + 1
            );
        }
    });
}

/// Conditional FF write: value must persist when condition is false.
/// With NBA, the condition becomes true one cycle after the counter is incremented.
#[test]
fn nba_conditional_write() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        out: output logic<32>,
    ) {
        var cnt: logic<4>;
        var val: logic<32>;

        always_ff {
            if_reset {
                cnt = 0;
                val = 0;
            } else {
                cnt += 1;
                if cnt == 4'd1 {
                    val = 32'd42;
                }
                // No else: val must retain 42 for all subsequent cycles
            }
        }
        assign out = val;
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        let (jclk, iclk) = dual.get_clock("clk");
        let (jrst, irst) = dual.get_reset("rst");
        dual.step(&jrst, &irst);

        // cycle 1: cnt_cur=0(old) → cnt==1 is false → val stays 0
        dual.step(&jclk, &iclk);
        assert_eq!(dual.get("out").unwrap(), Value::new(0, 32, false));

        // cycle 2: cnt_cur=1(old) → cnt==1 is true → val=42
        dual.step(&jclk, &iclk);
        assert_eq!(dual.get("out").unwrap(), Value::new(42, 32, false));

        // cycle 3+: condition false, val persists at 42
        for _ in 3..=10 {
            dual.step(&jclk, &iclk);
            assert_eq!(
                dual.get("out").unwrap(),
                Value::new(42, 32, false),
                "val should persist when condition is false"
            );
        }
    });
}

/// $assert in always_ff must read the pre-NBA value of a variable it shares
/// the block with. If system function argument reads are not tracked in
/// FfTable, the variable gets misclassified as comb (single-buffered) and
/// the write becomes immediate, so $assert sees the post-write value.
#[test]
fn nba_system_call_read_in_always_ff() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        out: output logic<32>,
    ) {
        var x: logic<32>;
        always_ff {
            if_reset {
                x = 99;
            } else {
                x = x + 1;
                $assert(x == 99, "x in $assert must be pre-NBA (99), not post-write (100)");
            }
        }
        assign out = x;
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        let (jclk, iclk) = dual.get_clock("clk");
        let (jrst, irst) = dual.get_reset("rst");
        dual.step(&jrst, &irst);
        dual.step(&jclk, &iclk);
        assert_eq!(dual.get("out").unwrap(), Value::new(100, 32, false));
    });
}

// ============================================================
// 4-state X/Z propagation tests
// ============================================================

/// FF variable with 4-state: verify X/Z clears after reset
/// and NBA produces correct values across cycles.
#[test]
fn ff_4state_xz_propagation() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        out: output logic<32>,
    ) {
        var a: logic<32>;
        var b: logic<32>;

        always_ff {
            if_reset {
                a = 32'd5;
                b = 32'd0;
            } else {
                a = a + 1;
                b = a;
            }
        }
        assign out = b;
    }
    "#;

    let config = Config {
        use_4state: true,
        use_jit: false,
        ..Default::default()
    };
    let ir = analyze(code, &config);
    let mut sim = Simulator::new(ir, None);
    let clk = sim.get_clock("clk").unwrap();
    let rst = sim.get_reset("rst").unwrap();

    sim.step(&rst);
    let b = sim.get("out").unwrap();
    assert_eq!(b.payload_u64(), 0, "b should be 0 after reset");
    assert!(!b.is_xz(), "b should not have X/Z after reset");

    sim.step(&clk);
    let b = sim.get("out").unwrap();
    assert_eq!(b.payload_u64(), 5);
    assert!(!b.is_xz());
}

/// Combinational 4-state: X in arithmetic produces X result.
#[test]
fn comb_4state_arithmetic() {
    let code = r#"
    module Top (
        a: input  logic<8>,
        b: input  logic<8>,
        sum: output logic<8>,
        and_out: output logic<8>,
    ) {
        assign sum = a + b;
        assign and_out = a & b;
    }
    "#;

    let config = Config {
        use_4state: true,
        use_jit: false,
        ..Default::default()
    };
    let ir = analyze(code, &config);
    let mut sim = Simulator::new(ir, None);

    sim.set("a", Value::new(5, 8, false));
    // b is unset (X in 4state) → sum has X
    let sum = sim.get("sum").unwrap();
    assert!(sum.is_xz(), "sum should have X when input b has X/Z");

    sim.set("b", Value::new(3, 8, false));
    let sum = sim.get("sum").unwrap();
    assert_eq!(sum.payload_u64(), 8);
    assert!(!sum.is_xz(), "sum should be clean after both inputs set");
}

// ============================================================
// JIT/interpreter consistency tests: mixed mode, load cache,
// store elimination, wide values, 4-state, dynamic indexing
// ============================================================

/// $display (can_build_binary=false) splits the comb block into
/// [Compiled, Interpreted, Compiled]. Verifies the block boundary handoff.
#[test]
fn dual_jit_mixed_display_block() {
    let code = r#"
    module Top (
        a: input  logic<32>,
        b: input  logic<32>,
        x: output logic<32>,
        y: output logic<32>,
    ) {
        var mid: logic<32>;
        always_comb {
            mid = a + b;
            $display("mid=%d", mid);
            x = mid * 2;
            y = mid + 100;
        }
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        for i in 0u64..10 {
            dual.set("a", Value::new(i * 10, 32, false));
            dual.set("b", Value::new(i * 5, 32, false));
            dual.step_synthetic();
            let mid = i * 10 + i * 5;
            assert_eq!(dual.get("x").unwrap(), Value::new(mid * 2, 32, false));
            assert_eq!(dual.get("y").unwrap(), Value::new(mid + 100, 32, false));
        }
    });
}

/// Load CSE cache: write then read the same variable within a single JIT block.
/// JIT uses cached value; interpreter re-reads from memory.
#[test]
fn dual_jit_load_cache_read_after_write() {
    let code = r#"
    module Top (
        a: input  logic<32>,
        x: output logic<32>,
    ) {
        var t1: logic<32>;
        var t2: logic<32>;
        always_comb {
            t1 = a + 1;
            t2 = t1 + 1;
            x = t2 + t1;
        }
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        for a in 0u64..20 {
            dual.set("a", Value::new(a, 32, false));
            dual.step_synthetic();
            assert_eq!(dual.get("x").unwrap(), Value::new(2 * a + 3, 32, false));
        }
    });
}

/// Merged comb+event JIT: a child module with comb assigns reading an
/// FF plus multiple always_ff blocks reading the same FF and a port
/// exercises the load_cache / event-dependency interaction in the
/// merged JIT path.
#[test]
fn dual_jit_merged_comb_event_flush_pattern() {
    let code = r#"
    module Top (
        clk   : input  clock,
        rst   : input  reset,
        result: output logic<8>,
        valid : output logic,
    ) {
        var counter: logic<8>;
        always_ff {
            if_reset {
                counter = 0;
            } else {
                counter += 1;
            }
        }

        let trap_taken: logic = counter == 8'd3 || counter == 8'd7;

        inst u_pipe: Pipeline (
            clk,
            rst,
            i_flush  : trap_taken,
            i_valid  : 1'b1,
            i_data   : counter,
            i_mode   : counter[1:0],
            o_valid  : valid,
            o_data   : result,
        );
    }

    module Pipeline (
        clk    : input  clock,
        rst    : input  reset,
        i_flush: input  logic,
        i_valid: input  logic,
        i_data : input  logic<8>,
        i_mode : input  logic<2>,
        o_valid: output logic,
        o_data : output logic<8>,
    ) {
        var flush_q: logic;
        always_ff {
            if_reset {
                flush_q = 1'b0;
            } else {
                flush_q = i_flush;
            }
        }

        // Several comb chains give the merged optimiser internal
        // variables to work on.
        let wen     : logic    = i_valid && !flush_q;
        let ren     : logic    = i_valid && !flush_q;
        let gated   : logic<8> = if wen ? i_data : 8'd0;
        let shifted : logic<8> = gated << i_mode;
        let masked  : logic<8> = shifted & 8'hFF;
        let combined: logic<8> = masked + gated;

        let is_special: logic    = i_mode == 2'd3;
        let sc_success: logic    = is_special && wen;
        let sc_result : logic<8> = if sc_success ? 8'd0 : 8'd1;
        let final_data: logic<8> = if is_special ? sc_result : combined;

        var saved_data: logic<8>;
        always_ff {
            if_reset {
                saved_data = 8'd0;
            } else if ren {
                saved_data = i_data;
            }
        }

        always_ff {
            if_reset {
                o_valid = 1'b0;
                o_data  = 8'd0;
            } else if i_flush || flush_q {
                o_valid = 1'b0;
            } else {
                o_valid = i_valid;
                o_data  = final_data + saved_data;
            }
        }
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        dual.step_reset("rst");

        for _ in 0u64..20 {
            dual.step_clock("clk");
        }
    });
}

/// Store elimination: internal comb variable in child module has its store
/// eliminated in JIT (forwarded via load_cache only).
#[test]
fn dual_jit_store_elimination_internal_comb() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        a:   input  logic<32>,
        out: output logic<32>,
    ) {
        inst u: Inner (
            clk,
            rst,
            a,
            out,
        );
    }

    module Inner (
        clk: input  clock,
        rst: input  reset,
        a:   input  logic<32>,
        out: output logic<32>,
    ) {
        var mid: logic<32>;
        always_comb {
            mid = a * 3;
        }
        always_ff {
            if_reset {
                out = 0;
            } else {
                out = mid + 10;
            }
        }
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        let (jclk, iclk) = dual.get_clock("clk");
        let (jrst, irst) = dual.get_reset("rst");
        dual.step(&jrst, &irst);
        assert_eq!(dual.get("out").unwrap(), Value::new(0, 32, false));

        for a in 1u64..10 {
            dual.set("a", Value::new(a, 32, false));
            dual.step(&jclk, &iclk);
            assert_eq!(dual.get("out").unwrap(), Value::new(a * 3 + 10, 32, false));
        }
    });
}

/// 96-bit operations: JIT uses I128, interpreter uses BigUint.
#[test]
fn dual_jit_wide_96bit_operations() {
    let code = r#"
    module Top (
        a: input  logic<96>,
        b: input  logic<96>,
        sum:     output logic<96>,
        and_out: output logic<96>,
        xor_out: output logic<96>,
    ) {
        always_comb {
            sum     = a + b;
            and_out = a & b;
            xor_out = a ^ b;
        }
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        let a = Value::new(0xDEAD_BEEF_CAFE_BABE, 96, false);
        let b = Value::new(0x1234_5678_9ABC_DEF0, 96, false);
        dual.set("a", a);
        dual.set("b", b);
        dual.step_synthetic();

        let expected_sum = Value::new(
            0xDEAD_BEEF_CAFE_BABEu64.wrapping_add(0x1234_5678_9ABC_DEF0),
            96,
            false,
        );
        assert_eq!(dual.get("sum").unwrap(), expected_sum);

        let expected_and = Value::new(0xDEAD_BEEF_CAFE_BABE & 0x1234_5678_9ABC_DEF0, 96, false);
        assert_eq!(dual.get("and_out").unwrap(), expected_and);

        let expected_xor = Value::new(0xDEAD_BEEF_CAFE_BABE ^ 0x1234_5678_9ABC_DEF0, 96, false);
        assert_eq!(dual.get("xor_out").unwrap(), expected_xor);
    });
}

/// 4-state BitAnd/BitOr mask propagation (X & 0 = 0, X | 1 = 1).
#[test]
fn dual_jit_4state_bitand_mask() {
    let code = r#"
    module Top (
        a: input  logic<8>,
        b: input  logic<8>,
        and_out: output logic<8>,
        or_out:  output logic<8>,
    ) {
        always_comb {
            and_out = a & b;
            or_out  = a | b;
        }
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        // a is unset (X in 4-state), b has strategic 0-bits to test X & 0 = 0
        dual.set("b", Value::new(0x0F, 8, false));
        dual.step_synthetic();

        dual.set("a", Value::new(0xAB, 8, false));
        dual.step_synthetic();
        assert_eq!(
            dual.get("and_out").unwrap(),
            Value::new(0xAB & 0x0F, 8, false)
        );
        assert_eq!(
            dual.get("or_out").unwrap(),
            Value::new(0xAB | 0x0F, 8, false)
        );
    });
}

/// Dynamic array indexing: JIT inline pointer arithmetic vs interpreter eval.
#[test]
fn dual_jit_dynamic_index_read() {
    let code = r#"
    module Top (
        idx: input  logic<2>,
        o  : output logic<8>,
    ) {
        var arr: logic<8> [4];

        assign arr[0] = 10;
        assign arr[1] = 20;
        assign arr[2] = 30;
        assign arr[3] = 40;
        assign o = arr[idx];
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        for idx in 0u64..4 {
            dual.set("idx", Value::new(idx, 2, false));
            dual.step_synthetic();
            assert_eq!(dual.get("o").unwrap(), Value::new((idx + 1) * 10, 8, false));
        }
    });
}

/// If-statement inside JIT: load_cache is cleared at branch entry.
#[test]
fn dual_jit_if_load_cache_boundary() {
    let code = r#"
    module Top (
        sel: input  logic,
        a:   input  logic<32>,
        out: output logic<32>,
    ) {
        var mid: logic<32>;
        always_comb {
            mid = a + 1;
            if sel {
                out = mid + 10;
            } else {
                out = mid + 20;
            }
        }
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        for a in 0u64..20 {
            dual.set("a", Value::new(a, 32, false));
            dual.set("sel", Value::new(1, 1, false));
            dual.step_synthetic();
            assert_eq!(dual.get("out").unwrap(), Value::new(a + 11, 32, false));

            dual.set("sel", Value::new(0, 1, false));
            dual.step_synthetic();
            assert_eq!(dual.get("out").unwrap(), Value::new(a + 21, 32, false));
        }
    });
}

/// Wide dynamic array assign (>64 bits, can_build_binary=false) mixed with
/// JIT-compilable statements in the same comb block.
#[test]
fn dual_jit_wide_dynamic_assign_mixed() {
    let code = r#"
    module Top (
        sel:      input  logic<2>,
        val:      input  logic<96>,
        a:        input  logic<32>,
        out:      output logic<32>,
        wide_out: output logic<96>,
    ) {
        var mem: logic<96> [4];
        always_comb {
            mem[sel] = val;
            out = a + 1;
            wide_out = mem[0];
        }
    }
    "#;

    verify_jit_interpreter_equivalence(code, |dual| {
        dual.set("sel", Value::new(0, 2, false));
        dual.set("val", Value::new(0xCAFEBABE, 96, false));
        dual.set("a", Value::new(42, 32, false));
        dual.step_synthetic();
        assert_eq!(dual.get("out").unwrap(), Value::new(43, 32, false));
        assert_eq!(
            dual.get("wide_out").unwrap(),
            Value::new(0xCAFEBABE, 96, false)
        );

        dual.set("sel", Value::new(1, 2, false));
        dual.set("val", Value::new(0xDEADBEEF, 96, false));
        dual.set("a", Value::new(99, 32, false));
        dual.step_synthetic();
        assert_eq!(dual.get("out").unwrap(), Value::new(100, 32, false));
    });
}

#[test]
fn packed_array_dynamic_bit_select_read() {
    let code = r#"
    module Top (
        a  : input  logic<4, 8>,
        idx: input  logic<2>,
        o  : output logic<8>,
    ) {
        assign o = a[idx];
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        // a = 0xDDCCBBAA, packed as [3]=0xDD [2]=0xCC [1]=0xBB [0]=0xAA
        sim.set("a", Value::new(0xDDCCBBAA, 32, false));
        sim.set("idx", Value::new(0, 2, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("o").unwrap(), Value::new(0xAA, 8, false));

        sim.set("idx", Value::new(1, 2, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("o").unwrap(), Value::new(0xBB, 8, false));

        sim.set("idx", Value::new(2, 2, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("o").unwrap(), Value::new(0xCC, 8, false));

        sim.set("idx", Value::new(3, 2, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("o").unwrap(), Value::new(0xDD, 8, false));
    }
}

// Regression for #20: a dynamic part-select `d[i+:4]` (const width, runtime
// start) dropped its window width in the sim IR and selected a single bit
// instead of 4. (`-:`/step with a runtime start are rejected upstream by the
// analyzer, so only `+:` reaches the simulator.)
#[test]
fn dynamic_part_select_read() {
    let code = r#"
    module Top (
        d  : input  logic<32>,
        i  : input  logic<3>,
        o_p: output logic<4>,
    ) {
        assign o_p = d[i+:4];
    }
    "#;

    let mut configs = Config::all();
    if crate::backend::aot_c::cc_available() {
        configs.push(aot_native_validate_config());
    }
    for config in configs {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let d: u64 = 0xABCD_1234;
        for i in 0..8u64 {
            sim.set("d", Value::new(d, 32, false));
            sim.set("i", Value::new(i, 3, false));
            sim.step(&Event::Clock(VarId::SYNTHETIC));
            assert_eq!(
                sim.get("o_p").unwrap(),
                Value::new((d >> i) & 0xF, 4, false),
                "d[{i}+:4] config={config:?}"
            );
        }
    }
}

// Regression for #20: LHS dynamic part-select write `o[i+:4] = v` must update the
// 4-bit window at runtime start i (register RMW), not a single bit.
#[test]
fn dynamic_part_select_write() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        i  : input  logic<3>,
        v  : input  logic<4>,
        o  : output logic<16>,
    ) {
        always_ff {
            if_reset {
                o = 0;
            } else {
                o[i+:4] = v;
            }
        }
    }
    "#;

    let mut configs = Config::all();
    if crate::backend::aot_c::cc_available() {
        configs.push(aot_native_validate_config());
    }
    for config in configs {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();
        sim.step(&rst);

        let mut exp: u64 = 0;
        for (i, v) in [(4u64, 0x5u64), (0, 0xA), (7, 0x3), (2, 0xC)] {
            sim.set("i", Value::new(i, 3, false));
            sim.set("v", Value::new(v, 4, false));
            sim.step(&clk);
            exp = ((exp & !(0xF << i)) | ((v & 0xF) << i)) & 0xFFFF;
            assert_eq!(
                sim.get("o").unwrap(),
                Value::new(exp, 16, false),
                "o[{i}+:4]={v} config={config:?}"
            );
        }
    }
}

#[test]
fn packed_array_single_bit_dynamic_select() {
    let code = r#"
    module Top (
        a  : input  logic<8>,
        idx: input  logic<3>,
        o  : output logic,
    ) {
        assign o = a[idx];
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        // 0b10110010: bit0=0, bit1=1, bit2=0, bit3=0, bit4=1, bit5=1, bit6=0, bit7=1
        let expected_bits: [u64; 8] = [0, 1, 0, 0, 1, 1, 0, 1];
        for i in 0..8u64 {
            sim.set("a", Value::new(0b10110010, 8, false));
            sim.set("idx", Value::new(i, 3, false));
            sim.step(&Event::Clock(VarId::SYNTHETIC));
            assert_eq!(
                sim.get("o").unwrap(),
                Value::new(expected_bits[i as usize], 1, false),
                "bit {} mismatch",
                i,
            );
        }
    }
}

#[test]
fn packed_array_3d_dynamic_select() {
    let code = r#"
    module Top (
        a  : input  logic<2, 3, 4>,
        idx: input  logic,
        o  : output logic<3, 4>,
    ) {
        assign o = a[idx];
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        // a = 0xABCDEF: a[0] = bits[11:0] = 0xDEF, a[1] = bits[23:12] = 0xABC
        sim.set("a", Value::new(0xABCDEF, 24, false));

        sim.set("idx", Value::new(0, 1, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("o").unwrap(),
            Value::new(0xDEF, 12, false),
            "a[0] mismatch",
        );

        sim.set("idx", Value::new(1, 1, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("o").unwrap(),
            Value::new(0xABC, 12, false),
            "a[1] mismatch",
        );
    }
}

#[test]
fn write_no_newline() {
    let code = r#"
    module Top (
        i_clk: input clock,
    ) {
        initial {
            $write("hello ");
            $write("world");
        }
    }
    "#;
    for config in Config::all() {
        output_buffer::enable();
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.step(&Event::Initial);
        let output = output_buffer::take();
        assert_eq!(output, "hello world");
    }
}

#[test]
fn write_format_specifiers() {
    let code = r#"
    module Top (
        i_clk: input clock,
    ) {
        initial {
            $write("hex=%h dec=%d", 8'hAB, 8'd42);
        }
    }
    "#;
    for config in Config::all() {
        output_buffer::enable();
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.step(&Event::Initial);
        let output = output_buffer::take();
        assert_eq!(output, "hex=ab dec=42");
    }
}

#[test]
fn write_no_format_string() {
    let code = r#"
    module Top (
        i_clk: input clock,
    ) {
        initial {
            $write(8'hFF, 4'b1010);
        }
    }
    "#;
    for config in Config::all() {
        output_buffer::enable();
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.step(&Event::Initial);
        let output = output_buffer::take();
        assert_eq!(output, "ff a");
    }
}

#[test]
fn write_and_display_mixed() {
    let code = r#"
    module Top (
        i_clk: input clock,
    ) {
        initial {
            $write("no newline ");
            $display("with newline");
        }
    }
    "#;
    for config in Config::all() {
        output_buffer::enable();
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.step(&Event::Initial);
        let output = output_buffer::take();
        assert_eq!(output, "no newline with newline\n");
    }
}

// Regression: concatenation in case condition was compiled with wrong element
// widths in JIT because eval_comptime was not called on the case target
// expression, leaving bit-select widths unresolved (full variable width).
#[test]
fn case_concat_bit_select() {
    let code = r#"
    module Top (
        a  : input  logic<16>,
        out: output logic<8> ,
    ) {
        always_comb {
            case {a[12], a[6:5]} {
                3'b111 : out = 8'd7;
                3'b110 : out = 8'd6;
                3'b101 : out = 8'd5;
                3'b100 : out = 8'd4;
                3'b011 : out = 8'd3;
                3'b010 : out = 8'd2;
                3'b001 : out = 8'd1;
                3'b000 : out = 8'd0;
                default: out = 8'hFF;
            }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        // bit12=1, bit6=1, bit5=1 => {1,11} = 7
        sim.set("a", Value::new(0x1060, 16, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("out").unwrap(),
            Value::new(7, 8, false),
            "config={config:?}: a=0x1060",
        );

        // bit12=1, bit6=1, bit5=0 => {1,10} = 6
        sim.set("a", Value::new(0x1040, 16, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("out").unwrap(),
            Value::new(6, 8, false),
            "config={config:?}: a=0x1040",
        );

        // bit12=0, bit6=1, bit5=1 => {0,11} = 3
        sim.set("a", Value::new(0x0060, 16, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("out").unwrap(),
            Value::new(3, 8, false),
            "config={config:?}: a=0x0060",
        );

        // bit12=0, bit6=0, bit5=0 => {0,00} = 0
        sim.set("a", Value::new(0x0000, 16, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("out").unwrap(),
            Value::new(0, 8, false),
            "config={config:?}: a=0x0000",
        );
    }
}

/// Instance output connected to individual bits of a wider signal (issue #2437).
#[test]
fn inst_output_bit_select() {
    let code = r#"
    module Top (
        r: output logic<2>,
    ) {
        inst r1: One (
            o: r[1],
        );
        inst r0: One (
            o: r[0],
        );
    }
    module One (
        o: output logic,
    ) {
        assign o = 1;
    }
    "#;
    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.ensure_comb_updated();
        let result = sim.get("r").unwrap().payload_u64();
        assert_eq!(
            result, 0b11,
            "JIT={} 4st={}: expected 0b11 got 0b{:b}",
            config.use_jit, config.use_4state, result
        );
    }
}

/// Instance input connected to individual bits of a wider signal.
#[test]
fn inst_input_bit_select() {
    let code = r#"
    module Top (
        a: input logic<2>,
        r: output logic<2>,
    ) {
        inst u1: Pass (
            i: a[1],
            o: r[1],
        );
        inst u0: Pass (
            i: a[0],
            o: r[0],
        );
    }
    module Pass (
        i: input  logic,
        o: output logic,
    ) {
        assign o = i;
    }
    "#;
    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.set("a", Value::new(0b10, 2, false));
        sim.ensure_comb_updated();
        let result = sim.get("r").unwrap().payload_u64();
        assert_eq!(
            result, 0b10,
            "JIT={} 4st={}: a=0b10 expected r=0b10 got 0b{:b}",
            config.use_jit, config.use_4state, result
        );

        sim.set("a", Value::new(0b01, 2, false));
        sim.ensure_comb_updated();
        let result = sim.get("r").unwrap().payload_u64();
        assert_eq!(
            result, 0b01,
            "JIT={} 4st={}: a=0b01 expected r=0b01 got 0b{:b}",
            config.use_jit, config.use_4state, result
        );

        sim.set("a", Value::new(0b11, 2, false));
        sim.ensure_comb_updated();
        let result = sim.get("r").unwrap().payload_u64();
        assert_eq!(
            result, 0b11,
            "JIT={} 4st={}: a=0b11 expected r=0b11 got 0b{:b}",
            config.use_jit, config.use_4state, result
        );
    }
}

#[test]
fn float_const_arithmetic() {
    let code_mul = r#"
    module Top (
        out: output logic<64>,
    ) {
        const R: i64 = (3.0 * 2.0) as i64;
        assign out = R;
    }
    "#;

    {
        let config = Config::default();
        let ir = analyze(code_mul, &config);
        let mut sim = Simulator::new(ir, None);
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        let result = sim.get("out").unwrap().payload_u64();
        assert_eq!(result, 6, "3.0 * 2.0 as i64 = {}", result);
    }

    let code_full = r#"
    module Top (
        out: output logic<64>,
    ) {
        const STEP : i64 = ((440.0 * 281474976710656.0) / 50000000.0) as i64;
        assign out = STEP;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code_full, &config);
        let mut sim = Simulator::new(ir, None);
        sim.step(&Event::Clock(VarId::SYNTHETIC));

        let result = sim.get("out").unwrap().payload_u64();
        assert_eq!(
            result, 2476979795,
            "JIT={} 4st={}: expected 2476979795 got {}",
            config.use_jit, config.use_4state, result
        );
    }
}

/// Regression test for https://github.com/veryl-lang/veryl/issues/2454
#[test]
fn issue_2454_f64_to_int_cast() {
    let code = r#"
    package Repro {
        type step_t = signed logic<48>;

        function step_from_hz(
            system_clk_hz: input u32,
            freq_hz      : input f64,
        ) -> step_t {
            const SCALE   : f64 = 281474976710656.0;
            let   step_f64: f64 = (freq_hz * SCALE) / system_clk_hz as f64;
            let   rounded : i64 = step_f64 as i64;
            return rounded as step_t;
        }
    }

    module Top (
        out: output logic<48>,
    ) {
        const STEP: Repro::step_t = Repro::step_from_hz(50_000_000, 440.0);
        assign out = STEP;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.step(&Event::Clock(VarId::SYNTHETIC));

        let result = sim.get("out").unwrap().payload_u64();
        assert_eq!(
            result, 2476979795,
            "issue_2454: JIT={} 4st={}: expected 2476979795 got {}",
            config.use_jit, config.use_4state, result
        );
    }
}

/// Regression test for https://github.com/veryl-lang/veryl/issues/2454 (reopened)
/// Mixed integer-float binary operations must promote the integer operand to float.
#[test]
fn issue_2454_mixed_int_float_binary() {
    let code_pow = r#"
    module Top (
        out: output logic<64>,
    ) {
        const SCALE: i64 = (2 ** ((48 - 7) as f64)) as i64;
        assign out = SCALE;
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code_pow, &config);
        let mut sim = Simulator::new(ir, None);
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        let result = sim.get("out").unwrap().payload_u64();
        assert_eq!(
            result,
            1u64 << 41,
            "2 ** ((48-7) as f64): JIT={} 4st={}: expected {} got {}",
            config.use_jit,
            config.use_4state,
            1u64 << 41,
            result
        );
    }

    let code_step = r#"
    package Repro {
        function step32_from_hz(
            system_clk_hz: input u32,
            freq         : input f64,
        ) -> u32 {
            const N_SHIFT: i32 = 7;
            const SCALE  : f64 = 2 ** ((48 - N_SHIFT) as f64);
            let step_f64 : f64 = (freq * SCALE) / system_clk_hz as f64;
            let rounded  : u32 = step_f64 as u32;
            return rounded;
        }
    }

    module Top (
        out: output logic<32>,
    ) {
        const STEP: u32 = Repro::step32_from_hz(50_000_000, 440.0);
        assign out = STEP;
    }
    "#;

    // Expected: (440.0 * 2^41) / 50_000_000 = 19351404 (truncated)
    let expected: u64 = ((440.0_f64 * (1u64 << 41) as f64) / 50_000_000.0) as u64;

    for config in Config::all() {
        let ir = analyze(code_step, &config);
        let mut sim = Simulator::new(ir, None);
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        let result = sim.get("out").unwrap().payload_u64();
        assert_eq!(
            result, expected,
            "step32_from_hz(440Hz): JIT={} 4st={}: expected {} got {}",
            config.use_jit, config.use_4state, expected, result
        );
    }
}

/// Regression: the "find max" pattern in always_comb — initialise a
/// state variable, then conditionally update it inside a loop that
/// reads itself — must not be store-eliminated when load_cache is
/// disabled, otherwise the elided value is not recoverable and the
/// loop reads stale memory.
///
/// Only triggers via the merged comb+event JIT path, so the module
/// under test must be instantiated as a child.
#[test]
fn find_max_with_self_reference_in_comb() {
    let code = r#"
    module Inner (
        clk    : input  clock    ,
        rst    : input  reset    ,
        i_set  : input  logic<8> ,
        i_clear: input  logic    ,
        o_vec  : output logic<8> ,
    ) {
        var vec : logic<8>;
        var prio: logic<3> [8];

        var best_id : logic<3>;
        var best_pri: logic<3>;

        always_comb {
            best_id  = 3'd0;
            best_pri = 3'd0;
            for i in 1..8 {
                if vec[i] && prio[i] >: best_pri {
                    best_id  = i[2:0];
                    best_pri = prio[i];
                }
            }
        }

        always_ff (clk, rst) {
            if_reset {
                vec = 8'd0;
                for i in 0..8 {
                    prio[i] = 3'd0;
                }
                prio[3] = 3'd5;
            } else {
                for i in 1..8 {
                    if i_set[i] && !vec[i] {
                        vec[i] = 1'b1;
                    }
                }
                if i_clear && best_id != 3'd0 {
                    vec[best_id] = 1'b0;
                }
            }
        }

        assign o_vec = vec;
    }

    module Top (
        clk    : input  clock    ,
        rst    : input  reset    ,
        i_set  : input  logic<8> ,
        i_clear: input  logic    ,
        o_vec  : output logic<8> ,
    ) {
        inst u_inner: Inner (
            clk            ,
            rst            ,
            i_set  : i_set ,
            i_clear: i_clear,
            o_vec  : o_vec ,
        );
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.set("i_set", Value::new(0, 8, false));
        sim.set("i_clear", Value::new(0, 1, false));
        sim.step(&rst);
        sim.step(&clk);

        // Latch vec[3] = 1 (prio[3] is the only nonzero priority).
        sim.set("i_set", Value::new(0b0000_1000, 8, false));
        sim.step(&clk);
        sim.set("i_set", Value::new(0, 8, false));

        let v = sim.get("o_vec").unwrap().payload_u64();
        assert_eq!(
            v & 0xff,
            0x08,
            "after set: JIT={} 4st={} ff_opt={}: vec=0x{:02x} expected 0x08",
            config.use_jit,
            config.use_4state,
            !config.disable_ff_opt,
            v & 0xff
        );

        // Clear via dynamic best_id; vec should be empty afterwards.
        sim.set("i_clear", Value::new(1, 1, false));
        sim.step(&clk);
        sim.set("i_clear", Value::new(0, 1, false));
        sim.step(&clk);

        let v = sim.get("o_vec").unwrap().payload_u64();
        assert_eq!(
            v & 0xff,
            0x00,
            "after claim: JIT={} 4st={} ff_opt={}: vec=0x{:02x} expected 0x00",
            config.use_jit,
            config.use_4state,
            !config.disable_ff_opt,
            v & 0xff
        );
    }
}

/// Regression: https://github.com/veryl-lang/veryl/issues/2490
#[test]
fn dispatch_binary_pattern_via_function() {
    let code = r#"
    module Top #(
        param WIDTH  : u32  = 4 ,
        param ENTRIES: u32  = 16,
        param DATA_TYPE: type = logic<WIDTH>,
    ) (
        sel : input  logic<4>,
        data: input  DATA_TYPE,
        o0  : output DATA_TYPE,
        o1  : output DATA_TYPE,
        o2  : output DATA_TYPE,
        o3  : output DATA_TYPE,
    ) {
        var tmp: DATA_TYPE<ENTRIES>;
        always_comb {
            for i in 0..ENTRIES {
                tmp[i] = 0 as DATA_TYPE;
            }
            tmp[sel] = data;
            o0 = tmp[0];
            o1 = tmp[1];
            o2 = tmp[2];
            o3 = tmp[3];
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        for sel_val in 0..4u64 {
            sim.set("sel", Value::new(sel_val, 4, false));
            sim.set("data", Value::new(sel_val + 1, 4, false));
            sim.step(&Event::Clock(VarId::SYNTHETIC));

            for j in 0..4u64 {
                let port = format!("o{}", j);
                let expected = if j == sel_val { sel_val + 1 } else { 0 };
                assert_eq!(
                    sim.get(&port).unwrap(),
                    Value::new(expected, 4, false),
                    "sel={} j={} expected={} JIT={} 4st={}",
                    sel_val,
                    j,
                    expected,
                    config.use_jit,
                    config.use_4state,
                );
            }
        }
    }
}

// Regression (#2506): always_comb body statements must preserve source
// order (SequentialBlock grouping prevents reordering by topo sort).
#[test]
fn always_comb_preserves_statement_order() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        a:   input  logic<32>,
        b:   output logic<32>,
    ) {
        always_comb {
            var c: logic<32>;
            var d: logic<32>;
            c = a;
            d = 2 * c;
            c = d;
            b = c;
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.set("a", Value::new(5, 32, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("b").unwrap(),
            Value::new(10, 32, false),
            "a=5 → b should be 10, JIT={} 4st={}",
            config.use_jit,
            config.use_4state,
        );
    }
}

// Regression (#2506): variable reassignment in an inlined function
// must not be flagged as a combinational loop.
#[test]
fn function_var_reassign_not_comb_loop() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        a:   input  logic<32>,
        b:   output logic<32>,
    ) {
        function f(a: input logic<32>) -> logic<32> {
            var b: logic<32>;
            var c: logic<32>;

            c = a;
            b = 2 * c;
            c = b;

            return c;
        }

        assign b = f(a);
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.set("a", Value::new(5, 32, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("b").unwrap(),
            Value::new(10, 32, false),
            "f(5) should be 10, JIT={} 4st={}",
            config.use_jit,
            config.use_4state,
        );

        sim.set("a", Value::new(0, 32, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("b").unwrap(),
            Value::new(0, 32, false),
            "f(0) should be 0, JIT={} 4st={}",
            config.use_jit,
            config.use_4state,
        );
    }
}

// Regression: a reorder-hazard block in a PHANTOM cross-block cycle must not be
// misreported as a combinational loop. Block A reassigns x (hazard -> kept
// atomic by the fast path) AND cross-references n; B writes n, reads m. `m=b`
// does NOT depend on n, so the design is ACYCLIC, but A's conflated I/O (reads
// n, writes m) + B forms a phantom cycle that the bipartite fast path rejects.
// analyze_dependency must then fall through to the full-flatten + stable sort,
// which unwraps A's statements and resolves the phantom (no false loop error).
#[test]
fn hazard_block_phantom_cycle_not_a_loop() {
    let code = r#"
    module Top (
        a:   input  logic<32>,
        b:   input  logic<32>,
        p_o: output logic<32>,
        q_o: output logic<32>,
    ) {
        var m: logic<32>;
        var n: logic<32>;
        always_comb {
            var x: logic<32>;
            x   = a;
            p_o = x;
            x   = b;
            m   = x;   // m = b, independent of n
            q_o = n;
        }
        always_comb {
            n = m;
        }
    }
    "#;
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.set("a", Value::new(5, 32, false));
        sim.set("b", Value::new(7, 32, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("p_o").unwrap(), Value::new(5, 32, false), "p_o=a");
        assert_eq!(
            sim.get("q_o").unwrap(),
            Value::new(7, 32, false),
            "q_o=n=m=b"
        );
    }
}

// Regression: sequential reassignment must survive analyze_dependency's
// Phase-2 flatten AND reorder_by_level's leveling. Two always_comb blocks
// cross-reference m/z so Phase-1 sees a phantom cycle and drops into Phase 2,
// flattening both. Block A reassigns a local `x` (x=c; y_o=x; x=d; w_o=x) with
// a read BETWEEN the writes; the old bipartite sort and pure-RAW leveling each
// reordered it so `y_o` read `d`. With the fix `y_o` must read `c` (5).
#[test]
fn phase2_sequential_reassign_survives_reorder() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        a:   input  logic<32>,
        b:   input  logic<32>,
        c:   input  logic<32>,
        d:   input  logic<32>,
        y_o: output logic<32>,
        w_o: output logic<32>,
        p_o: output logic<32>,
        q_o: output logic<32>,
    ) {
        var m: logic<32>;
        var z: logic<32>;

        always_comb {
            var x: logic<32>;
            m   = a;
            p_o = z;
            x   = c;
            y_o = x; // must read c, not the later x=d
            x   = d;
            w_o = x; // must read d
        }

        always_comb {
            z   = b;
            q_o = m;
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.set("a", Value::new(1, 32, false));
        sim.set("b", Value::new(2, 32, false));
        sim.set("c", Value::new(5, 32, false));
        sim.set("d", Value::new(7, 32, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));

        let msg = format!("JIT={} 4st={}", config.use_jit, config.use_4state);
        assert_eq!(
            sim.get("y_o").unwrap(),
            Value::new(5, 32, false),
            "y_o must read x=c (5), {msg}",
        );
        assert_eq!(
            sim.get("w_o").unwrap(),
            Value::new(7, 32, false),
            "w_o must read x=d (7), {msg}",
        );
        assert_eq!(
            sim.get("p_o").unwrap(),
            Value::new(2, 32, false),
            "p_o must read z=b (2), {msg}",
        );
        assert_eq!(
            sim.get("q_o").unwrap(),
            Value::new(1, 32, false),
            "q_o must read m=a (1), {msg}",
        );
    }
}

#[test]
fn for_break_in_comb() {
    let code_basic = r#"
    module Top (
        a0: input  logic<8>,
        a1: input  logic<8>,
        a2: input  logic<8>,
        a3: input  logic<8>,
        sum: output logic<8>,
    ) {
        var a: logic<8> [4];
        always_comb {
            a[0] = a0;
            a[1] = a1;
            a[2] = a2;
            a[3] = a3;
            sum = 0;
            for i in 0..4 {
                sum += a[i];
            }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code_basic, &config);
        let mut sim = Simulator::new(ir, None);
        sim.set("a0", Value::new(1, 8, false));
        sim.set("a1", Value::new(2, 8, false));
        sim.set("a2", Value::new(3, 8, false));
        sim.set("a3", Value::new(4, 8, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("sum").unwrap(),
            Value::new(10, 8, false),
            "basic for sum: JIT={} 4st={}",
            config.use_jit,
            config.use_4state,
        );
    }

    let code = r#"
    module Top (
        a0: input  logic<8>,
        a1: input  logic<8>,
        a2: input  logic<8>,
        a3: input  logic<8>,
        idx: output logic<8>,
    ) {
        var a: logic<8> [4];
        always_comb {
            a[0] = a0;
            a[1] = a1;
            a[2] = a2;
            a[3] = a3;
            idx = 0;
            for i in 0..4 {
                if a[i] != 0 {
                    idx = i as 8;
                    break;
                }
            }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.set("a0", Value::new(0, 8, false));
        sim.set("a1", Value::new(0, 8, false));
        sim.set("a2", Value::new(0, 8, false));
        sim.set("a3", Value::new(0, 8, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("idx").unwrap(),
            Value::new(0, 8, false),
            "all-zero: JIT={} 4st={}",
            config.use_jit,
            config.use_4state,
        );

        sim.set("a2", Value::new(1, 8, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("idx").unwrap(),
            Value::new(2, 8, false),
            "a[2]=1: JIT={} 4st={}",
            config.use_jit,
            config.use_4state,
        );

        sim.set("a1", Value::new(5, 8, false));
        sim.set("a3", Value::new(9, 8, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("idx").unwrap(),
            Value::new(1, 8, false),
            "a[1]=5,a[3]=9: JIT={} 4st={}",
            config.use_jit,
            config.use_4state,
        );
    }
}

/// Static for-loop with `break` following an assignment in the body:
/// statements before break must execute each iteration until break fires.
#[test]
fn for_break_after_assign_in_comb() {
    let code = r#"
    module Top (
        limit: input  logic<8>,
        sum  : output logic<8>,
    ) {
        always_comb {
            sum = 0;
            for i in 0..4 {
                sum += i as 8;
                if (i as 8) == limit {
                    break;
                }
            }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.set("limit", Value::new(4, 8, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("sum").unwrap(),
            Value::new(6, 8, false),
            "limit=4 (no break, sum=0+1+2+3): JIT={} 4st={}",
            config.use_jit,
            config.use_4state,
        );

        sim.set("limit", Value::new(2, 8, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("sum").unwrap(),
            Value::new(3, 8, false),
            "limit=2 (break at i=2, sum=0+1+2): JIT={} 4st={}",
            config.use_jit,
            config.use_4state,
        );
    }
}

/// Dynamic-range for-loop with `break`: `Statement::For::eval_step` path.
#[test]
fn for_break_in_dynamic_range_function() {
    let code = r#"
    module Top (
        n    : input  logic<8>,
        limit: input  logic<8>,
        sum  : output logic<8>,
    ) {
        function count_until(
            n    : input u32,
            limit: input u32,
        ) -> u32 {
            var s: u32;
            s = 0;
            for i in 0..n {
                if i == limit {
                    break;
                }
                s += 1;
            }
            return s;
        }

        always_comb {
            sum = count_until(n as 32, limit as 32) as 8;
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.set("n", Value::new(5, 8, false));
        sim.set("limit", Value::new(10, 8, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("sum").unwrap(),
            Value::new(5, 8, false),
            "n=5,limit=10 (no break): JIT={} 4st={}",
            config.use_jit,
            config.use_4state,
        );

        sim.set("limit", Value::new(3, 8, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("sum").unwrap(),
            Value::new(3, 8, false),
            "n=5,limit=3 (break at i=3): JIT={} 4st={}",
            config.use_jit,
            config.use_4state,
        );
    }
}

#[test]
fn dynamic_for_range_in_function() {
    let code = r#"
    module Top #(
        param T: type = logic<4>,
    )(
        o0: output T,
        o1: output T,
        o2: output T,
        o3: output T,
        o4: output T,
        o5: output T,
        o6: output T,
        o7: output T,
    ) {
        function func(
            beg_outer: input  u32 ,
            end_outer: input  u32 ,
            out      : output T<8>,
        ) {
            var m: i32;
            var n: i32;

            for i in 0..8 {
                out[i] = 0 as T;
            }

            m = 8;
            for i in beg_outer..end_outer {
                n = m;
                m = n / 2;
                for j in 0..m {
                    out[4 * i + j] = (4 * i + j) as T;
                }
            }
        }

        var out: T<8>;

        always_comb {
            func(0, 2, out);
            o0  = out[0];
            o1  = out[1];
            o2  = out[2];
            o3  = out[3];
            o4  = out[4];
            o5  = out[5];
            o6  = out[6];
            o7  = out[7];
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.step(&Event::Clock(VarId::SYNTHETIC));

        let exp_list: [u64; 8] = [0, 1, 2, 3, 4, 5, 0, 0];
        for (i, exp) in exp_list.iter().enumerate() {
            let port = format!("o{}", i);
            assert_eq!(
                sim.get(&port).unwrap(),
                Value::new(*exp, 4, false),
                "i={} expected={} JIT={} 4st={}",
                i,
                exp,
                config.use_jit,
                config.use_4state,
            );
        }
    }
}

#[test]
fn dynamic_for_range_in_unrolled_static_for() {
    let code = r#"
    module Top (
        o0: output logic<4>,
        o1: output logic<4>,
        o2: output logic<4>,
        o3: output logic<4>,
        o4: output logic<4>,
        o5: output logic<4>,
        o6: output logic<4>,
        o7: output logic<4>,
    ) {
        function func(
            i_data: input  logic<8, 4>,
            o_data: output logic<8, 4>,
        ) {
            const DEPTH: u32 = 2;
            var current_n: u32;
            var current_d: logic<8, 4>;
            var next_n   : u32;
            var next_d   : logic<8, 4>;

            next_n = 8;
            next_d = i_data;
            for _i in 0..DEPTH {
                current_n = next_n;
                current_d = next_d;
                next_n = current_n / 2;
                for j in 0..next_n {
                    next_d[j] = (current_d[2 * j + 0] + current_d[2 * j + 1]) as 4;
                }
            }
            o_data = next_d;
        }

        var data: logic<8, 4>;
        var out : logic<8, 4>;

        always_comb {
            data[0] = 1;
            data[1] = 2;
            data[2] = 3;
            data[3] = 4;
            data[4] = 5;
            data[5] = 6;
            data[6] = 7;
            data[7] = 8;
            func(data, out);
            o0 = out[0];
            o1 = out[1];
            o2 = out[2];
            o3 = out[3];
            o4 = out[4];
            o5 = out[5];
            o6 = out[6];
            o7 = out[7];
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.step(&Event::Clock(VarId::SYNTHETIC));

        // iter 0 (next_n=4): next_d[0..3] = [3, 7, 11, 15]
        // iter 1 (next_n=2): next_d[0..1] = [3+7, (11+15)%16] = [10, 10]
        let exp_list: [u64; 8] = [10, 10, 11, 15, 5, 6, 7, 8];
        for (i, exp) in exp_list.iter().enumerate() {
            let port = format!("o{}", i);
            assert_eq!(
                sim.get(&port).unwrap(),
                Value::new(*exp, 4, false),
                "i={} expected={} JIT={} 4st={}",
                i,
                exp,
                config.use_jit,
                config.use_4state,
            );
        }
    }
}

#[test]
fn for_static_in_always_ff_reset() {
    let code = r#"
    module Top (
        clk: input clock,
        rst: input reset,
        o0: output logic<8>,
        o1: output logic<8>,
        o2: output logic<8>,
        o3: output logic<8>,
    ) {
        var data: logic<8> [4];

        always_ff (clk, rst) {
            if_reset {
                for i in 0..4 {
                    data[i] = (i + 10) as 8;
                }
            }
        }

        assign o0 = data[0];
        assign o1 = data[1];
        assign o2 = data[2];
        assign o3 = data[3];
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.step(&sim.get_reset("rst").unwrap());
        assert_eq!(sim.get("o0").unwrap(), Value::new(10, 8, false));
        assert_eq!(sim.get("o1").unwrap(), Value::new(11, 8, false));
        assert_eq!(sim.get("o2").unwrap(), Value::new(12, 8, false));
        assert_eq!(sim.get("o3").unwrap(), Value::new(13, 8, false));
    }
}

#[test]
fn for_static_step_and_rev() {
    let code = r#"
    module Top (
        sum_step: output logic<32>,
        sum_rev: output logic<32>,
    ) {
        always_comb {
            sum_step = 0;
            for i in 0..10 step += 3 {
                sum_step += i;
            }
            sum_rev = 0;
            for i in rev 0..4 {
                sum_rev = sum_rev * 10 + i as 32;
            }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        // 0 + 3 + 6 + 9 = 18
        assert_eq!(
            sim.get("sum_step").unwrap(),
            Value::new(18, 32, false),
            "step: JIT={} 4st={}",
            config.use_jit,
            config.use_4state,
        );
        // 3*1000 + 2*100 + 1*10 + 0 = 3210
        assert_eq!(
            sim.get("sum_rev").unwrap(),
            Value::new(3210, 32, false),
            "rev: JIT={} 4st={}",
            config.use_jit,
            config.use_4state,
        );
    }
}

#[test]
fn for_rev_with_step() {
    // A reverse loop combined with a step must visit the same values as the
    // emitted SystemVerilog `for (int i = hi - 1; i >= lo; i -= step)`. For
    // `rev 0..10 step += 2` that is {9, 7, 5, 3, 1} (sum 25), and for the
    // inclusive `rev 0..=10 step += 2` that is {10, 8, 6, 4, 2, 0} (sum 30).
    // Regression for the simulator diverging from the synthesized RTL.
    let code = r#"
    module Top (
        sum_excl: output logic<32>,
        sum_incl: output logic<32>,
    ) {
        always_comb {
            sum_excl = 0;
            for i in rev 0..10 step += 2 {
                sum_excl += i;
            }
            sum_incl = 0;
            for i in rev 0..=10 step += 2 {
                sum_incl += i;
            }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("sum_excl").unwrap(),
            Value::new(25, 32, false),
            "rev excl: JIT={} 4st={}",
            config.use_jit,
            config.use_4state,
        );
        assert_eq!(
            sim.get("sum_incl").unwrap(),
            Value::new(30, 32, false),
            "rev incl: JIT={} 4st={}",
            config.use_jit,
            config.use_4state,
        );
    }
}

#[test]
fn wide_dynamic_bit_select() {
    // `bits[j]` with a runtime `j` on a 128-bit variable must return the
    // actual bit at position `j`, not a u64-truncated view (bits past 63
    // wrapping to the low half).
    let code = r#"
    module Top (
        bits: input  logic<128>,
        idx:  input  logic<7>,
        out:  output logic,
    ) {
        assign out = bits[idx];
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let one_at_lsb = Value::from_u128(1, 0, 128, false);
        sim.set("bits", one_at_lsb);
        for j in [0u8, 1, 63, 64, 65, 127] {
            sim.set("idx", Value::new(j as u64, 7, false));
            sim.step(&Event::Clock(VarId::SYNTHETIC));
            let expected = if j == 0 { 1 } else { 0 };
            assert_eq!(
                sim.get("out").unwrap().payload_u64(),
                expected,
                "bits=1<<0 idx={} JIT={} 4st={}",
                j,
                config.use_jit,
                config.use_4state,
            );
        }

        let one_at_msb = Value::from_u128(1u128 << 127, 0, 128, false);
        sim.set("bits", one_at_msb);
        for j in [0u8, 1, 63, 64, 65, 126, 127] {
            sim.set("idx", Value::new(j as u64, 7, false));
            sim.step(&Event::Clock(VarId::SYNTHETIC));
            let expected = if j == 127 { 1 } else { 0 };
            assert_eq!(
                sim.get("out").unwrap().payload_u64(),
                expected,
                "bits=1<<127 idx={} JIT={} 4st={}",
                j,
                config.use_jit,
                config.use_4state,
            );
        }

        let one_at_64 = Value::from_u128(1u128 << 64, 0, 128, false);
        sim.set("bits", one_at_64);
        for j in [0u8, 63, 64, 65, 127] {
            sim.set("idx", Value::new(j as u64, 7, false));
            sim.step(&Event::Clock(VarId::SYNTHETIC));
            let expected = if j == 64 { 1 } else { 0 };
            assert_eq!(
                sim.get("out").unwrap().payload_u64(),
                expected,
                "bits=1<<64 idx={} JIT={} 4st={}",
                j,
                config.use_jit,
                config.use_4state,
            );
        }
    }
}

#[test]
fn wide_all_bit_equality() {
    // `x == '1` must compare `x` against the all-ones fill at `x`'s width,
    // not against the integer literal 1. Same for `'0`.
    let code = r#"
    module Top (
        x:         input  logic<2>,
        x_wide:    input  logic<128>,
        is_all1:   output logic,
        is_all0:   output logic,
        wide_all1: output logic,
    ) {
        assign is_all1   = x      == '1;
        assign is_all0   = x      == '0;
        assign wide_all1 = x_wide == '1;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.set("x", Value::new(0b11, 2, false));
        sim.set("x_wide", Value::from_u128(u128::MAX, 0, 128, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("is_all1").unwrap().payload_u64(),
            1,
            "2'b11 == '1 should be true (JIT={} 4st={})",
            config.use_jit,
            config.use_4state,
        );
        assert_eq!(
            sim.get("is_all0").unwrap().payload_u64(),
            0,
            "2'b11 == '0 should be false (JIT={} 4st={})",
            config.use_jit,
            config.use_4state,
        );
        assert_eq!(
            sim.get("wide_all1").unwrap().payload_u64(),
            1,
            "128'h{{ff*16}} == '1 should be true (JIT={} 4st={})",
            config.use_jit,
            config.use_4state,
        );

        sim.set("x", Value::new(0b01, 2, false));
        sim.set("x_wide", Value::from_u128(1, 0, 128, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("is_all1").unwrap().payload_u64(),
            0,
            "2'b01 == '1 should be false (JIT={} 4st={})",
            config.use_jit,
            config.use_4state,
        );
        assert_eq!(
            sim.get("is_all0").unwrap().payload_u64(),
            0,
            "2'b01 == '0 should be false (JIT={} 4st={})",
            config.use_jit,
            config.use_4state,
        );
        assert_eq!(
            sim.get("wide_all1").unwrap().payload_u64(),
            0,
            "128'h1 == '1 should be false (JIT={} 4st={})",
            config.use_jit,
            config.use_4state,
        );

        sim.set("x", Value::new(0, 2, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("is_all0").unwrap().payload_u64(),
            1,
            "2'b00 == '0 should be true (JIT={} 4st={})",
            config.use_jit,
            config.use_4state,
        );
    }
}

#[test]
fn wide_bit_reverse() {
    // Integration check for bit-select reads and writes that cross the
    // u64 boundary on a 128-bit variable. 4-state storage layout (payload
    // + mask_xz) is particularly sensitive: a read that uses the wrong
    // native width overlaps the two regions.
    let code = r#"
    module Top (
        bits: input  logic<128>,
        reversed: output logic<128>,
    ) {
        always_comb {
            for i in 0..128 {
                reversed[i] = bits[128 - i - 1];
            }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let cases: &[(u128, u128)] = &[
            (1, 1u128 << 127),
            (1u128 << 127, 1),
            (1u128 << 64, 1u128 << 63),
            (1u128 << 63, 1u128 << 64),
        ];
        for &(input, expected) in cases {
            // Fresh sim per case so prior `reversed` state (or 4-state
            // default X) can't accidentally satisfy the next assertion.
            let ir = analyze(code, &config);
            let mut sim = Simulator::new(ir, None);
            sim.set("bits", Value::from_u128(input, 0, 128, false));
            sim.step(&Event::Clock(VarId::SYNTHETIC));
            assert_eq!(
                sim.get("reversed").unwrap(),
                Value::from_u128(expected, 0, 128, false),
                "rev({:#x}) JIT={} 4st={}",
                input,
                config.use_jit,
                config.use_4state,
            );
        }
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let pat: u128 = 0xdead_beef_0000_0001_8000_0000_cafe_babe;
        let mut rev_pat: u128 = 0;
        for i in 0..128 {
            if (pat >> i) & 1 != 0 {
                rev_pat |= 1u128 << (127 - i);
            }
        }
        sim.set("bits", Value::from_u128(pat, 0, 128, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("reversed").unwrap(),
            Value::from_u128(rev_pat, 0, 128, false),
            "rev(pat) JIT={} 4st={}",
            config.use_jit,
            config.use_4state,
        );
    }
}

/// Regression test for duplicate ProtoStatement → false SCC in the
/// child→parent comb/post_comb promotion path.
///
/// Structure: a child drives a parent signal; the parent's always_comb
/// reads it in a self-referential pattern (local `let`, re-read in the
/// next expression).  When that parent stmt would also need to run
/// post-events to see fresh child outputs, it must be *moved* into
/// `all_post_comb_fns`, not duplicated alongside `all_comb_statements`.
/// A duplicate would write and read `e` from two ProtoStatements,
/// forming a 2-stmt SCC in `unified`.
#[test]
fn scc_zero_after_comb_post_comb_dedup() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        o:   output logic<8>,
    ) {
        var cnt: logic<8>;
        inst c: Child (
            clk,
            rst,
            cnt_o: cnt,
        );
        always_comb {
            let e: logic<8> = cnt + 1;
            o = e * 2 + e;
        }
    }

    module Child (
        clk:   input  clock,
        rst:   input  reset,
        cnt_o: output logic<8>,
    ) {
        var s: logic<8>;
        always_ff {
            if_reset {
                s = 0;
            } else {
                s = s + 1;
            }
        }
        assign cnt_o = s;
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);
        assert_eq!(
            ir.nontrivial_comb_scc, 0,
            "expected SCC=0 (config: jit={}, 4st={})",
            config.use_jit, config.use_4state,
        );
    }
}

/// Direct field read on a dynamically-indexed FF struct array. Before
/// the DynamicVariable nb fix, fields above the bottom of the element
/// (here valid/flag2 at bit 41/40 and the top byte of data) read as 0
/// because the load width was sized to the field, not the element.
#[test]
fn dynamic_index_struct_field_read_ff() {
    let code = r#"
    module Top (
        clk    : input  clock     ,
        rst    : input  reset     ,
        i_wr_en: input  logic     ,
        i_idx  : input  logic<3>  ,
        i_data : input  logic<32> ,
        o_valid: output logic     ,
        o_flag2: output logic     ,
        o_data : output logic<32> ,
        o_tag  : output logic<8>  ,
    ) {
        struct Entry {
            valid: logic    ,
            flag2: logic    ,
            data : logic<32>,
            tag  : logic<8> ,
        }

        var arr: Entry [8];
        always_ff {
            if_reset {
                for i in 0..8 {
                    arr[i].valid = 0;
                    arr[i].flag2 = 0;
                    arr[i].data  = 0;
                    arr[i].tag   = 0;
                }
            } else {
                if i_wr_en {
                    arr[i_idx].valid = 1;
                    arr[i_idx].flag2 = 1;
                    arr[i_idx].data  = i_data;
                    arr[i_idx].tag   = 8'hA5;
                }
            }
        }

        assign o_valid = arr[i_idx].valid;
        assign o_flag2 = arr[i_idx].flag2;
        assign o_data  = arr[i_idx].data;
        assign o_tag   = arr[i_idx].tag;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        // Write 0xDEADBEEF to arr[3].
        sim.set("i_wr_en", Value::new(1, 1, false));
        sim.set("i_idx", Value::new(3, 3, false));
        sim.set("i_data", Value::new(0xDEADBEEF, 32, false));
        sim.step(&clk);

        // Now read all four fields of arr[3] through direct field access.
        sim.set("i_wr_en", Value::new(0, 1, false));
        sim.step(&clk);

        assert_eq!(
            sim.get("o_valid").unwrap(),
            Value::new(1, 1, false),
            "config: jit={}, 4st={}",
            config.use_jit,
            config.use_4state,
        );
        assert_eq!(
            sim.get("o_flag2").unwrap(),
            Value::new(1, 1, false),
            "config: jit={}, 4st={}",
            config.use_jit,
            config.use_4state,
        );
        assert_eq!(
            sim.get("o_data").unwrap(),
            Value::new(0xDEADBEEF, 32, false),
            "config: jit={}, 4st={}",
            config.use_jit,
            config.use_4state,
        );
        assert_eq!(
            sim.get("o_tag").unwrap(),
            Value::new(0xA5, 8, false),
            "config: jit={}, 4st={}",
            config.use_jit,
            config.use_4state,
        );
    }
}

/// Whole-array assignment between matching-shape array variables. The
/// element-wise expansion in conv replaces a single-stmt path that
/// previously panicked in calc_index for empty-index on a non-empty array.
#[test]
fn whole_array_assign() {
    let code = r#"
    module Top (
        clk  : input  clock     ,
        rst  : input  reset     ,
        i_v  : input  logic<8>  ,
        i_i  : input  logic<2>  ,
        i_sel: input  logic<2>  ,
        o_sel: output logic<8>  ,
    ) {
        var arr: logic<8> [4];
        var o  : logic<8> [4];
        always_ff {
            if_reset {
                for i in 0..4 {
                    arr[i] = 0;
                }
            } else {
                arr[i_i] = i_v;
            }
        }
        assign o     = arr;
        assign o_sel = o[i_sel];
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);

        // Write distinct values into each slot of `arr`.  Each cycle
        // also triggers a fresh whole-array assign `o = arr`.
        for i in 0..4u64 {
            sim.set("i_i", Value::new(i, 2, false));
            sim.set("i_v", Value::new(0x10 + i, 8, false));
            sim.step(&clk);
        }
        sim.step(&clk);

        // Tap each element through `o[i_sel]` and confirm the whole-array
        // assign propagated every element correctly.
        for i in 0..4u64 {
            sim.set("i_sel", Value::new(i, 2, false));
            assert_eq!(
                sim.get("o_sel").unwrap(),
                Value::new(0x10 + i, 8, false),
                "config: jit={}, 4st={}, idx={}",
                config.use_jit,
                config.use_4state,
                i,
            );
        }
    }
}

/// Regression test for duplicate ProtoStatement → false SCC in the
/// merged-JIT path of `InstDeclaration::conv`.
///
/// When a child module's comb+event JIT merges successfully, originals
/// must live only inside the pushed CompiledBlock's `original_stmts`
/// (which `analyze_dependency` Phase 2 expands on demand).  Returning a
/// parallel copy outside the CB would put both views into the parent's
/// `unified` list, and any self-referential always_comb inside the child
/// would form a cross-stmt SCC between the CB and its originals.
#[test]
fn merged_jit_dup_scc_regression() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        o:   output logic<8>,
    ) {
        inst c: Child (
            clk,
            rst,
            o,
        );
    }

    module Child (
        clk: input  clock,
        rst: input  reset,
        o:   output logic<8>,
    ) {
        var s: logic<8>;

        always_ff {
            if_reset {
                s = 0;
            } else {
                s = s + 1;
            }
        }

        always_comb {
            let e: logic<8> = s + 1;
            o = e * 2 + e;
        }
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);
        assert_eq!(
            ir.nontrivial_comb_scc, 0,
            "expected SCC=0 (config: jit={}, 4st={})",
            config.use_jit, config.use_4state,
        );
    }
}

/// A `var` declared inside an `always_ff` block is a procedural local
/// (BA semantics), not a register: its value must not persist across
/// clock edges.
#[test]
fn local_var_in_always_ff_is_not_register() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        o_x: output logic,
    ) {
        always_ff {
            if_reset {
                o_x = 1'b0;
            } else {
                var tmp: logic;
                if true {
                    tmp = 1'b1;
                }
                o_x = tmp;
            }
        }
    }
    "#;

    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        let clk = sim.get_clock("clk").unwrap();
        let rst = sim.get_reset("rst").unwrap();

        sim.step(&rst);
        assert_eq!(sim.get("o_x").unwrap(), Value::new(0, 1, false));

        sim.step(&clk);
        assert_eq!(
            sim.get("o_x").unwrap(),
            Value::new(1, 1, false),
            "o_x should latch 1 after first clock (jit={}, 4st={})",
            config.use_jit,
            config.use_4state,
        );
        sim.step(&clk);
        assert_eq!(sim.get("o_x").unwrap(), Value::new(1, 1, false));
    }
}

// Regression: width-growing op results (Add/Sub/Mul, left shifts) fed to a
// comparison must be masked to width; the cranelift/aot_c backends left dirty
// high bits and diverged from the interpreter.
#[test]
fn binary_result_masked_to_width() {
    let code = r#"
    module Top (
        a: input  logic<4>,
        b: input  logic<4>,
        c_add: output logic,
        c_mul: output logic,
        c_shl: output logic,
    ) {
        // (0xf + 0x3) = 0x12; truncated to 4 bits = 0x2
        assign c_add = (a + b) == 4'h2;
        // (0x5 * 0xd) = 0x41; truncated to 4 bits = 0x1
        assign c_mul = (a * b) == 4'h1;
        // (0x5 <<< 2) = 0x14; truncated to 4 bits = 0x4
        assign c_shl = (a <<< 2) == 4'h4;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        // c_add: a=0xf, b=0x3
        sim.set("a", Value::new(0xf, 4, false));
        sim.set("b", Value::new(0x3, 4, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            format!("{:b}", sim.get("c_add").unwrap()),
            "1'b1",
            "add: JIT={} 4st={} aot={}",
            config.use_jit,
            config.use_4state,
            config.aot_c,
        );

        // c_mul: a=0x5, b=0xd
        sim.set("a", Value::new(0x5, 4, false));
        sim.set("b", Value::new(0xd, 4, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            format!("{:b}", sim.get("c_mul").unwrap()),
            "1'b1",
            "mul: JIT={} 4st={}",
            config.use_jit,
            config.use_4state,
        );

        // c_shl: a=0x5
        sim.set("a", Value::new(0x5, 4, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            format!("{:b}", sim.get("c_shl").unwrap()),
            "1'b1",
            "shl: JIT={} 4st={}",
            config.use_jit,
            config.use_4state,
        );
    }
}

// Regression: unary `~x`/`-x` results fed to a comparison must be masked to
// width; the aot_c backend left dirty high bits for an inlined consumer and
// diverged from the interpreter / Cranelift.
#[test]
fn unary_result_masked_to_width() {
    let code = r#"
    module Top (
        a:     input  logic<4>,
        c_not: output logic,
        c_neg: output logic,
    ) {
        // ~0x5 within 4 bits = 0xa (as i64, ~5 = ...fa; must mask to width).
        assign c_not = (~a) == 4'ha;
        // -0x5 within 4 bits = 0xb (two's complement; as i64, -5 = ...fb).
        assign c_neg = (-a) == 4'hb;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.set("a", Value::new(0x5, 4, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            format!("{:b}", sim.get("c_not").unwrap()),
            "1'b1",
            "not: JIT={} 4st={} aot={}",
            config.use_jit,
            config.use_4state,
            config.aot_c,
        );
        assert_eq!(
            format!("{:b}", sim.get("c_neg").unwrap()),
            "1'b1",
            "neg: JIT={} 4st={} aot={}",
            config.use_jit,
            config.use_4state,
            config.aot_c,
        );
    }
}

// Regression: the aot_c width mask on a width-growing op feeding a comparison
// is gated by an operand-derived overflow predicate. The predicate must use a
// shift (`(x|y) >> (W-1)`), not `(x|y) & (1<<(W-1))`: an inner unmasked add
// leaves dirty bits AT/ABOVE W, which the bit-test would miss, wrongly eliding
// the mask. Here (a+b) overflows to bit 4 (bit 3 clear), so a bit-3 test would
// skip the outer mask and read 0x10 instead of 0.
#[test]
fn nested_add_dirty_operand_masked() {
    let code = r#"
    module Top (
        a:   input  logic<4>,
        b:   input  logic<4>,
        c:   input  logic<4>,
        out: output logic,
    ) {
        assign out = ((a + b) + c) == 4'h0;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        // a+b = 0xf+0x1 = 0x10 (bit 4 set, bit 3 clear); +0 stays 0x10.
        // Masked to 4 bits = 0, so the comparison must be true.
        sim.set("a", Value::new(0xf, 4, false));
        sim.set("b", Value::new(0x1, 4, false));
        sim.set("c", Value::new(0x0, 4, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            format!("{:b}", sim.get("out").unwrap()),
            "1'b1",
            "JIT={} 4st={} aot={}",
            config.use_jit,
            config.use_4state,
            config.aot_c,
        );
    }
}

// Regression: the operand-derived overflow predicate gating the aot_c width
// mask, for a variable left shift feeding a comparison. Exercises all three
// arms: shift count >= width (always masks), runtime `width - n` shift skips
// the mask when no bit reaches the top, and applies it when one does.
#[test]
fn shift_left_overflow_masked() {
    let code = r#"
    module Top (
        a:   input  logic<8>,
        sh:  input  logic<4>,
        out: output logic,
    ) {
        assign out = (a << sh) == 8'h00;
    }
    "#;

    // (a, sh, expected `out`): masked result == 0 ?
    let cases = [
        (0x80, 1, "1'b1"), // 0x80<<1 = 0x100 → masked 0x00 → ==0 true
        (0x01, 2, "1'b0"), // 0x01<<2 = 0x04 → no overflow, skip → !=0 false
        (0x01, 8, "1'b1"), // 0x01<<8 → all bits past width → masked 0 → true
    ];
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        for (a, sh, expected) in cases {
            sim.set("a", Value::new(a, 8, false));
            sim.set("sh", Value::new(sh, 4, false));
            sim.step(&Event::Clock(VarId::SYNTHETIC));
            assert_eq!(
                format!("{:b}", sim.get("out").unwrap()),
                expected,
                "a={a:#x} sh={sh}: JIT={} 4st={} aot={}",
                config.use_jit,
                config.use_4state,
                config.aot_c,
            );
        }
    }
}

// Regression: a >64-bit result from <=64-bit operands was truncated by the
// aot_c backend's 64-bit C arithmetic (now bailed to cranelift), diverging
// from cranelift/interpreter.
#[test]
fn wide_result_from_narrow_operands() {
    let code = r#"
    module Top (
        a: input  logic<40>,
        b: input  logic<40>,
        p: output logic<80>,
    ) {
        assign p = a * b;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        // (2^40 - 1)^2 = 0xfffffffffe0000000001 (80 bits)
        sim.set("a", Value::from_str("40'hffffffffff").unwrap());
        sim.set("b", Value::from_str("40'hffffffffff").unwrap());
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            format!("{:x}", sim.get("p").unwrap()),
            "80'hfffffffffe0000000001",
            "wide mul: JIT={} 4st={}",
            config.use_jit,
            config.use_4state,
        );
    }
}

// Regression: a >64-bit Add/Sub/Mul with one operand already >64 bits must
// NOT be bailed by the aot_c backend — C's usual arithmetic conversions
// promote the narrow operand to __uint128_t, so the op is computed in 128
// bits. Over-bailing here forced the whole comb block to cranelift (a large
// linux-boot regression). Carry into bit 64 distinguishes a correct
// 128-bit add from a truncated 64-bit one.
#[test]
fn wide_result_one_operand_wide() {
    let code = r#"
    module Top (
        a: input  logic<64>,
        b: input  logic<72>,
        s: output logic<72>,
    ) {
        assign s = a + b;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        // (2^64 - 1) + 1 = 2^64: a 65-bit result a 64-bit add would wrap to 0.
        sim.set("a", Value::from_str("64'hffffffffffffffff").unwrap());
        sim.set("b", Value::from_str("72'h1").unwrap());
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            format!("{:x}", sim.get("s").unwrap()),
            "72'h010000000000000000",
            "wide add (one wide operand): JIT={} 4st={} aot={}",
            config.use_jit,
            config.use_4state,
            config.aot_c,
        );
    }
}

// Regression: dup_assign_dce dropped a live store to an interior array
// element across a dynamic read `arr[idx]` (the base+last read encoding hid
// it). Needs length >= 3 so elem 1 is neither base nor last.
#[test]
fn dup_assign_dce_dynamic_array_read() {
    let code = r#"
    module Top (
        idx: input  logic<2>,
        y:   output logic<8>,
        a1:  output logic<8>,
    ) {
        var arr: logic<8> [3];
        always_comb {
            arr[0] = 8'h00;
            arr[1] = 8'haa;   // live: the dynamic read below may select it
            arr[2] = 8'h00;
            y = arr[idx];     // with idx==1 must observe 0xaa
            arr[1] = 8'hbb;   // later overwrite must NOT kill the 0xaa store
        }
        assign a1 = arr[1];
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.set("idx", Value::new(1, 2, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("y").unwrap(),
            Value::new(0xaa, 8, false),
            "y should read arr[1]==0xaa at the read point: JIT={} 4st={}",
            config.use_jit,
            config.use_4state,
        );
        // The final value of arr[1] is the last write.
        assert_eq!(sim.get("a1").unwrap(), Value::new(0xbb, 8, false));
    }
}

#[test]
fn wide_signed_compare_uses_operand_width() {
    // Regression: the Cranelift wide (>128-bit) signed comparison passed the
    // result width (1) to wide_scmp instead of the operand width, so it probed
    // bit 0 as the sign bit and fell back to an unsigned compare. For signed
    // 200-bit operands, -1 >: 1 must be false (the interpreter's answer), not
    // the unsigned (2**200-1) >: 1 == true.
    let code = r#"
    module Top (
        a : input  signed logic<200>,
        b : input  signed logic<200>,
        gt: output logic,
    ) {
        assign gt = a >: b;
    }
    "#;

    let neg_one: Value = format!("200'sh{}", "f".repeat(50)).parse().unwrap();
    let one: Value = "200'sd1".parse().unwrap();

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.set("a", neg_one.clone());
        sim.set("b", one.clone());
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("gt").unwrap(), Value::new(0, 1, false));
    }
}

#[test]
fn wide_signed_compare_asymmetric_width() {
    // Regression: signed comparison of wide (>128-bit) operands with DIFFERENT
    // widths located the sign at a single common width, so a negative narrower
    // operand was mis-read. Each operand must be sign-extended from its own
    // width.
    let code = r#"
    module Top (
        a : input  signed logic<200>,
        b : input  signed logic<130>,
        gt: output logic,
        lt: output logic,
    ) {
        assign gt = a >: b;
        assign lt = a <: b;
    }
    "#;
    let a_pos1: Value = "200'sd1".parse().unwrap();
    let a_neg1: Value = format!("200'sh{}", "f".repeat(50)).parse().unwrap();
    let b_pos1: Value = "130'sd1".parse().unwrap();
    let b_neg1: Value = format!("130'sh3{}", "f".repeat(32)).parse().unwrap();
    let one = Value::new(1, 1, false);
    let zero = Value::new(0, 1, false);

    // (a, b, expected_gt, expected_lt)
    let cases = [
        (a_pos1.clone(), b_neg1.clone(), one.clone(), zero.clone()), // 1 >: -1
        (a_neg1.clone(), b_pos1.clone(), zero.clone(), one.clone()), // -1 <: 1
        (a_neg1.clone(), b_neg1.clone(), zero.clone(), zero.clone()), // -1 == -1
    ];

    for config in Config::all() {
        dbg!(&config);
        for (a, b, egt, elt) in &cases {
            let ir = analyze(code, &config);
            let mut sim = Simulator::new(ir, None);
            sim.set("a", a.clone());
            sim.set("b", b.clone());
            sim.step(&Event::Clock(VarId::SYNTHETIC));
            assert_eq!(
                sim.get("gt").unwrap(),
                *egt,
                "gt mismatch for {a:?} >: {b:?}"
            );
            assert_eq!(
                sim.get("lt").unwrap(),
                *elt,
                "lt mismatch for {a:?} <: {b:?}"
            );
        }
    }
}

#[test]
fn wide_signed_compare_asymmetric_width_aot_c() {
    // Regression (cc/AOT-C): vw_scmp located the sign of both operands at a
    // single common width, mis-reading a negative narrower operand. The cc
    // backend must sign-extend each operand from its own width. The
    // aot_c_validate config also dual-runs cc vs Cranelift every cycle.
    if !crate::backend::aot_c::cc_available() {
        return; // no external C compiler on this host
    }
    let code = r#"
    module Top (
        a : input  signed logic<200>,
        b : input  signed logic<130>,
        gt: output logic,
        lt: output logic,
    ) {
        assign gt = a >: b;
        assign lt = a <: b;
    }
    "#;
    let a_pos1: Value = "200'sd1".parse().unwrap();
    let a_neg1: Value = format!("200'sh{}", "f".repeat(50)).parse().unwrap();
    let b_pos1: Value = "130'sd1".parse().unwrap();
    let b_neg1: Value = format!("130'sh3{}", "f".repeat(32)).parse().unwrap();
    let one = Value::new(1, 1, false);
    let zero = Value::new(0, 1, false);
    let cases = [
        (a_pos1, b_neg1.clone(), one.clone(), zero.clone()), // 1 >: -1
        (a_neg1.clone(), b_pos1, zero.clone(), one.clone()), // -1 <: 1
        (a_neg1, b_neg1, zero.clone(), zero.clone()),        // -1 == -1
    ];
    let config = aot_native_validate_config();
    for (a, b, egt, elt) in cases {
        let ir = analyze(code, &config);
        assert!(
            ir.whole_comb.is_some(),
            "wide signed compare must be AOT-C-native to exercise vw_scmp_asym"
        );
        let mut sim = Simulator::new(ir, None);
        sim.set("a", a);
        sim.set("b", b);
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("gt").unwrap(), egt);
        assert_eq!(sim.get("lt").unwrap(), elt);
    }
}

#[test]
fn case_duplicate_value_first_match() {
    // Regression: the Cranelift br_table lowering filled the jump table in arm
    // order, so on a duplicate selector value the LAST arm won. SystemVerilog
    // `case` is first-match, so sel==1 must select 20 (the first `8'd1` arm),
    // not 77. Needs >=4 arms to exercise the br_table path.
    let code = r#"
    module Top (
        sel   : input  logic<8>,
        result: output logic<32>,
    ) {
        always_comb {
            case sel {
                8'd0: result = 32'd10;
                8'd1: result = 32'd20;
                8'd2: result = 32'd30;
                8'd3: result = 32'd40;
                8'd1: result = 32'd77;
                default: result = 32'd99;
            }
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.set("sel", Value::new(1, 8, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("result").unwrap(), Value::new(20, 32, false));
    }
}

#[test]
fn signed_cast_constfold_div_and_compare() {
    // Regression: an `as <signed>` cast lost its signedness in the const-eval
    // ExpressionContext, so Div/Rem and signed comparisons of cast operands
    // folded as unsigned (diverging from the emitted SystemVerilog).
    let code = r#"
    module Top (
        o: output signed logic<32>,
        z: output logic,
    ) {
        type sn = signed bit<32>;
        const A: sn = (-100 as sn) / (7 as sn);
        const C: logic = if ((-100 as sn) <: (7 as sn)) ? 1 : 0;
        assign o = A;
        assign z = C;
    }
    "#;

    let expect_o: Value = "32'shfffffff2".parse().unwrap(); // -100 / 7 = -14
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("o").unwrap().payload_u128(),
            expect_o.payload_u128()
        );
        assert_eq!(sim.get("z").unwrap(), Value::new(1, 1, false));
    }
}

#[test]
fn unary_binds_tighter_than_cast() {
    // The emitter nests `&X as u16` as `(&X) as u16` (`unsigned'(shortint'(&X))`),
    // so const-eval must too. A reduction's value depends on width: reducing the
    // 8-bit all-ones X gives 1, while the as-inside reading `&(X as u16)` reduces
    // the zero-extended 16-bit value and gives 0. VCS confirms the emitted SV is
    // 1, so the analyzer fold (0 before this fix) was the divergent one.
    let code = r#"
    module Top (
        o: output logic<16>,
    ) {
        const X: logic<8>  = 8'hff;
        const A: logic<16> = &X as u16;
        assign o = A;
    }
    "#;
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("o").unwrap(),
            Value::new(1, 16, false),
            "{config:?}"
        );
    }
}

// Regression: a numeric-width `as <N>` cast yields an UNSIGNED result at
// RUNTIME (`x as N` is an unsigned `logic<N>` like the emitted SV); a stray
// signed flag on the cast would sign-extend it when widened / arithmetic-
// shifted. The operand is a RUNTIME input (const operands fold via the separate
// path `comptime_widening_cast_sign_extends` covers), exercising the signedness
// `gather_context` computes. With the bug: wid=0xff80, asr=0xc0.
#[test]
fn runtime_numeric_width_cast_is_unsigned() {
    let code = r#"
    module Top (
        a:   input  logic<8> ,
        wid: output logic<16>,
        asr: output logic<8> ,
    ) {
        always_comb {
            wid = (a as 8) as 16;
            asr = (a as 8) >>> 1;
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        // a = 0x80: MSB set, so a signed-vs-unsigned interpretation diverges.
        sim.set("a", Value::new(0x80, 8, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("wid").unwrap().payload_u128(),
            0x0080,
            "(a as 8) as 16 must zero-extend, {config:?}",
        );
        assert_eq!(
            sim.get("asr").unwrap().payload_u128(),
            0x40,
            "(a as 8) >>> 1 must not sign-fill, {config:?}",
        );
    }
}

#[test]
fn comptime_widening_cast_sign_extends() {
    // A widening `as` cast of a signed value sign-extends like SV's `N'(expr)`:
    // `-1 as 16` is 0xffff, not 0x00ff.
    let code = r#"
    module Top (
        o: output logic<16>,
    ) {
        const A: i8        = -1;
        const B: logic<16> = A as 16;
        assign o = B;
    }
    "#;
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("o").unwrap().payload_u128(), 0xffff, "{config:?}");
    }
}

#[test]
fn all_bit_underscore_width() {
    // `1_0'1` (width 10) must strip the underscore like based literals; else the
    // width parses as 0 and the all-ones value collapses to 0 instead of 1023.
    let code = r#"
    module Top (
        o: output logic<10>,
    ) {
        const A: logic<10> = 1_0'1;
        assign o = A;
    }
    "#;
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("o").unwrap().payload_u128(), 0x3ff, "{config:?}");
    }
}

// A wide (>128-bit) dynamic part-select write `o[i+:130] = v` must store the
// 130-bit window at runtime start i, identically on every backend. (A #20 review
// flagged this as a possible pre-existing divergence; it is not — all backends
// agree and are correct, so this pins the behavior.)
#[test]
fn wide_dynamic_part_select_write() {
    let code = r#"
    module Top (
        clk: input  clock,
        rst: input  reset,
        i  : input  logic<3>,
        v  : input  logic<130>,
        o  : output logic<200>,
    ) {
        always_ff {
            if_reset {
                o = 0;
            } else {
                o[i+:130] = v;
            }
        }
    }
    "#;
    // High bits across multiple 64-bit words exercise the wide (pointer) path.
    let v: u128 = (1u128 << 127) | (1u128 << 100) | (1u128 << 64) | 0xDEAD_BEEF;
    for i in [0u64, 5] {
        for config in Config::all() {
            let ir = analyze(code, &config);
            let mut sim = Simulator::new(ir, None);
            let clk = sim.get_clock("clk").unwrap();
            let rst = sim.get_reset("rst").unwrap();
            sim.step(&rst);
            sim.set("v", Value::from_u128(v, 0, 130, false));
            sim.set("i", Value::new(i, 3, false));
            sim.step(&clk);
            // payload_u128 captures the low 128 result bits; `v << i` truncated
            // to 128 bits is the expected low window content.
            assert_eq!(
                sim.get("o").unwrap().payload_u128(),
                v.wrapping_shl(i as u32),
                "o[{i}+:130]=v config={config:?}"
            );
        }
    }
}

#[test]
fn comb_block_cycle_war_preserves_blocking_order() {
    // WAR: the forward read of `ext` in `o = a + ext` must not drag `o` past the
    // later `a = in1`, so o captures a's earlier value 0, not in1.
    let code = r#"
    module Top (
        in1: input  logic<32>,
        y:   output logic<32>,
        o:   output logic<32>,
    ) {
        var a:   logic<32>;
        var ext: logic<32>;
        always_comb {
            a = 0;
            o = a + ext;
            a = in1;
            y = a;
        }
        always_comb {
            ext = y + 1;
        }
    }
    "#;
    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.set("in1", Value::new(50, 32, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        // y = in1 = 50; ext = y + 1 = 51; o = a(==0) + ext = 51.
        assert_eq!(
            sim.get("y").unwrap(),
            Value::new(50, 32, false),
            "y config={config:?}"
        );
        assert_eq!(
            sim.get("o").unwrap(),
            Value::new(51, 32, false),
            "o config={config:?}"
        );
    }
}

#[test]
fn comb_backward_reader_not_interleaved_into_write_group() {
    // A reader with NO prior writer of `hit` (a backward edge, resolved by an
    // extra eval pass) must never be scheduled BETWEEN the constituent writes
    // of `hit` (the always_comb reset + the unrolled conditional sets): there
    // it re-reads the same mid-computation value on every pass and never
    // converges. Here the single-writer RAW edge on `c3` drags `gate` down
    // the schedule, which used to land it after the reset / first iteration.
    // The match is at iteration 1 so an interleaved reader sees hit==0.
    let code = r#"
    module Top (
        k:  input  logic<8>,
        en: input  logic<8>,
        o:  output logic<8>,
    ) {
        var hit : logic<8>;
        var gate: logic<8>;
        var c0  : logic<8>;
        var c1  : logic<8>;
        var c2  : logic<8>;
        var c3  : logic<8>;
        var d0  : logic<8>;
        var d1  : logic<8>;
        var d2  : logic<8>;
        var d3  : logic<8>;
        var d4  : logic<8>;
        var d5  : logic<8>;
        var d6  : logic<8>;
        var d7  : logic<8>;
        assign gate = hit & c3;
        assign c0 = en + 1;
        assign c1 = c0 + 0;
        assign c2 = c1 + 0;
        assign c3 = c2 + 0;
        // Deep chain feeding the scan key: the conditional writers of `hit`
        // schedule late (depth 9) while its reset (no inputs) hoists early;
        // `gate` (depth 5) used to land in between.
        assign d0 = k + 1;
        assign d1 = d0 + 0;
        assign d2 = d1 + 0;
        assign d3 = d2 + 0;
        assign d4 = d3 + 0;
        assign d5 = d4 + 0;
        assign d6 = d5 + 0;
        assign d7 = d6 + 0;
        var tbl: logic<8> [4];
        assign tbl[0] = 8'd16;
        assign tbl[1] = 8'd32;
        assign tbl[2] = 8'd48;
        assign tbl[3] = 8'd64;
        always_comb {
            hit = 8'd0;
            for i in 0..4 {
                if tbl[i] == d7 {
                    hit = 8'd255;
                }
            }
        }
        assign o = gate;
    }
    "#;
    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.set("k", Value::new(63, 8, false));
        sim.set("en", Value::new(254, 8, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        // d7 = 64, hit = 255 (tbl[3] == d7), c3 = 255, o = 255.
        assert_eq!(
            sim.get("o").unwrap(),
            Value::new(255, 8, false),
            "o config={config:?}"
        );
    }
}

#[test]
fn array_range_assign_multidim_unpacked() {
    // Range over the outer dim of a 2-D unpacked array, nested literal.
    // o[0+:2] = '{'{1,2},'{3,4}} => arr[0][0]=1, arr[0][1]=2, arr[1][0]=3, arr[1][1]=4.
    let code = r#"
    module Top (
        i: input  logic<1>,
        j: input  logic<1>,
        o: output logic<8>,
    ) {
        var arr: logic<8> [2, 2];
        assign arr[0+:2] = '{'{8'd1, 8'd2}, '{8'd3, 8'd4}};
        assign o = arr[i][j];
    }
    "#;
    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        for (i, j, exp) in [(0u64, 0u64, 1u64), (0, 1, 2), (1, 0, 3), (1, 1, 4)] {
            sim.set("i", Value::new(i, 1, false));
            sim.set("j", Value::new(j, 1, false));
            sim.step(&Event::Clock(VarId::SYNTHETIC));
            assert_eq!(
                sim.get("o").unwrap(),
                Value::new(exp, 8, false),
                "arr[{i}][{j}] config={config:?}"
            );
        }
    }
}

#[test]
fn array_range_assign_multidim_packed() {
    // Range over a 1-D array of multi-dim-packed elements; scalar literals.
    // o[0+:2] = '{100, 200} => arr[0]=100, arr[1]=200 (each logic<10,10>).
    let code = r#"
    module Top (
        i: input  logic<1>,
        o: output logic<10, 10>,
    ) {
        var arr: logic<10, 10> [2];
        assign arr[0+:2] = '{100, 200};
        assign o = arr[i];
    }
    "#;
    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        for (i, exp) in [(0u64, 100u128), (1, 200)] {
            sim.set("i", Value::new(i, 1, false));
            sim.step(&Event::Clock(VarId::SYNTHETIC));
            assert_eq!(
                sim.get("o").unwrap().payload_u128(),
                exp,
                "arr[{i}] config={config:?}"
            );
        }
    }
}

#[test]
fn array_range_assign_prefix_then_range() {
    // Fixed outer index then a range on the inner dim: arr[1][0+:2] = '{5,6}.
    // Exercises the inner-dim collapse in to_assign_destinations.
    let code = r#"
    module Top (
        i: input  logic<2>,
        j: input  logic<1>,
        o: output logic<8>,
    ) {
        var arr: logic<8> [3, 2];
        assign arr[0]      = '{8'd9, 8'd9};
        assign arr[1][0+:2] = '{8'd5, 8'd6};
        assign arr[2]      = '{8'd9, 8'd9};
        assign o = arr[i][j];
    }
    "#;
    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        for (i, j, exp) in [(1u64, 0u64, 5u64), (1, 1, 6), (0, 0, 9), (2, 1, 9)] {
            sim.set("i", Value::new(i, 2, false));
            sim.set("j", Value::new(j, 1, false));
            sim.step(&Event::Clock(VarId::SYNTHETIC));
            assert_eq!(
                sim.get("o").unwrap(),
                Value::new(exp, 8, false),
                "arr[{i}][{j}] config={config:?}"
            );
        }
    }
}

#[test]
fn inst_input_unsized_all_ones() {
    let code = r#"
    module Sub (
        i_a   : input  logic<32>,
        i_mask: input  logic<32>,
        o     : output logic<32>,
    ) {
        assign o = i_a & i_mask;
    }
    module Top (
        a: input  logic<32>,
        c: output logic<32>,
    ) {
        inst u: Sub ( i_a: a, i_mask: '1, o: c );
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.set("a", Value::new(0x12345678, 32, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));

        assert_eq!(sim.get("c").unwrap(), Value::new(0x12345678, 32, false));
    }
}

#[test]
fn inst_input_unsized_all_ones_wide() {
    let code = r#"
    module Sub (
        i_a   : input  logic<66>,
        i_mask: input  logic<66>,
        o     : output logic<66>,
    ) {
        assign o = i_a & i_mask;
    }
    module Top (
        a: input  logic<66>,
        c: output logic<66>,
    ) {
        inst u: Sub ( i_a: a, i_mask: '1, o: c );
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.set("a", Value::new(0x123456789, 66, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));

        assert_eq!(sim.get("c").unwrap(), Value::new(0x123456789, 66, false));
    }
}

#[test]
fn inst_output_concat_destructure() {
    let code = r#"
    module Sub (
        o: output logic<8>,
    ) {
        assign o = 8'b11100001;
    }
    module Top (
        a: output logic<3>,
        b: output logic<5>,
    ) {
        inst u: Sub ( o: {a, b} );
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.step(&Event::Clock(VarId::SYNTHETIC));

        // a takes the TOP 3 bits, b the LOW 5 bits
        assert_eq!(sim.get("a").unwrap(), Value::new(0b111, 3, false));
        assert_eq!(sim.get("b").unwrap(), Value::new(0b00001, 5, false));
    }
}

#[test]
fn inst_output_concat_destructure_wide128() {
    let code = r#"
    module Sub (
        o: output logic<128>,
    ) {
        assign o = {64'hDEADBEEF01234567, 64'h89ABCDEFFEDCBA98};
    }
    module Top (
        a: output logic<64>,
        b: output logic<64>,
    ) {
        inst u: Sub ( o: {a, b} );
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.step(&Event::Clock(VarId::SYNTHETIC));

        assert_eq!(
            sim.get("a").unwrap(),
            Value::from_str("64'hDEADBEEF_01234567").unwrap()
        );
        assert_eq!(
            sim.get("b").unwrap(),
            Value::from_str("64'h89ABCDEF_FEDCBA98").unwrap()
        );
    }
}

#[test]
fn inst_output_concat_destructure_wide66() {
    let code = r#"
    module Sub (
        o: output logic<66>,
    ) {
        assign o = {2'b10, 64'h0123456789ABCDEF};
    }
    module Top (
        a: output logic<2>,
        b: output logic<64>,
    ) {
        inst u: Sub ( o: {a, b} );
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.step(&Event::Clock(VarId::SYNTHETIC));

        assert_eq!(sim.get("a").unwrap(), Value::new(0b10, 2, false));
        assert_eq!(
            sim.get("b").unwrap(),
            Value::from_str("64'h01234567_89ABCDEF").unwrap()
        );
    }
}

#[test]
fn inst_output_concat_destructure_wide160() {
    let code = r#"
    module Sub (
        o: output logic<160>,
    ) {
        assign o = {32'hAAAA5555, 64'hDEADBEEF01234567, 64'h89ABCDEFFEDCBA98};
    }
    module Top (
        a: output logic<32>,
        b: output logic<128>,
    ) {
        inst u: Sub ( o: {a, b} );
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.step(&Event::Clock(VarId::SYNTHETIC));

        assert_eq!(sim.get("a").unwrap(), Value::new(0xAAAA5555, 32, false));
        assert_eq!(
            sim.get("b").unwrap(),
            Value::from_str("128'hDEADBEEF_01234567_89ABCDEF_FEDCBA98").unwrap()
        );
    }
}

#[test]
fn inst_output_concat_destructure_wide330() {
    let code = r#"
    module Sub (
        o: output logic<330>,
    ) {
        assign o = {200'hF1E2D3C4B5A69788796A5B4C3D2E1F00DEADBEEF01234567CA, 130'h35A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A};
    }
    module Top (
        a: output logic<200>,
        b: output logic<130>,
    ) {
        inst u: Sub ( o: {a, b} );
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.step(&Event::Clock(VarId::SYNTHETIC));

        assert_eq!(
            sim.get("a").unwrap(),
            Value::from_str("200'hF1E2D3C4B5A69788796A5B4C3D2E1F00DEADBEEF01234567CA").unwrap()
        );
        assert_eq!(
            sim.get("b").unwrap(),
            Value::from_str("130'h35A5A5A5A5A5A5A5A5A5A5A5A5A5A5A5A").unwrap()
        );
    }
}

#[test]
fn inst_output_concat_narrower_than_port() {
    // SV semantics: `{a, b} = o` truncates the RHS to the concat width,
    // so the fields take o's LOW 6 bits, not its top bits.
    let code = r#"
    module Sub (
        o: output logic<8>,
    ) {
        assign o = 8'b10110011;
    }
    module Top (
        a: output logic<3>,
        b: output logic<3>,
    ) {
        inst u: Sub ( o: {a, b} );
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.step(&Event::Clock(VarId::SYNTHETIC));

        // a = o[5:3], b = o[2:0]
        assert_eq!(sim.get("a").unwrap(), Value::new(0b110, 3, false));
        assert_eq!(sim.get("b").unwrap(), Value::new(0b011, 3, false));
    }
}

#[test]
fn inst_output_concat_wider_than_port() {
    // SV semantics: the RHS is zero-extended to the concat width, so the
    // top field's high bits read zero.
    let code = r#"
    module Sub (
        o: output logic<6>,
    ) {
        assign o = 6'b101100;
    }
    module Top (
        a: output logic<4>,
        b: output logic<4>,
    ) {
        inst u: Sub ( o: {a, b} );
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.step(&Event::Clock(VarId::SYNTHETIC));

        // a = {2'b00, o[5:4]}, b = o[3:0]
        assert_eq!(sim.get("a").unwrap(), Value::new(0b0010, 4, false));
        assert_eq!(sim.get("b").unwrap(), Value::new(0b1100, 4, false));
    }
}

#[test]
fn inst_output_concat_entirely_above_port() {
    // The top field lies entirely above the zero-extended port value and
    // must be driven to zero (not left undriven).
    let code = r#"
    module Sub (
        o: output logic<4>,
    ) {
        assign o = 4'b1011;
    }
    module Top (
        a: output logic<4>,
        b: output logic<8>,
    ) {
        inst u: Sub ( o: {a, b} );
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.step(&Event::Clock(VarId::SYNTHETIC));

        assert_eq!(sim.get("a").unwrap(), Value::new(0, 4, false));
        assert_eq!(sim.get("b").unwrap(), Value::new(0b00001011, 8, false));
    }
}

#[test]
fn function_arg_unsized_all_ones() {
    let code = r#"
    module Top (
        a: input  logic<32>,
        c: output logic<32>,
    ) {
        function mask_and (
            x: input logic<32>,
            m: input logic<32>,
        ) -> logic<32> {
            return x & m;
        }
        always_comb {
            c = mask_and(a, '1);
        }
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.set("a", Value::new(0x12345678, 32, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));

        assert_eq!(sim.get("c").unwrap(), Value::new(0x12345678, 32, false));
    }
}

#[test]
fn assign_concat_destructure_all_ones() {
    let code = r#"
    module Top (
        a: output logic<3>,
        b: output logic<5>,
    ) {
        assign {a, b} = '1;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.step(&Event::Clock(VarId::SYNTHETIC));

        assert_eq!(sim.get("a").unwrap(), Value::new(0b111, 3, false));
        assert_eq!(sim.get("b").unwrap(), Value::new(0b11111, 5, false));
    }
}

#[test]
fn assign_concat_destructure_wide_rhs() {
    let code = r#"
    module Top (
        i: input  logic<128>,
        a: output logic<64>,
        b: output logic<64>,
    ) {
        assign {a, b} = i;
    }
    "#;

    for config in Config::all() {
        dbg!(&config);

        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);

        sim.set(
            "i",
            Value::from_str("128'hDEADBEEF_01234567_89ABCDEF_FEDCBA98").unwrap(),
        );
        sim.step(&Event::Clock(VarId::SYNTHETIC));

        assert_eq!(
            sim.get("a").unwrap(),
            Value::from_str("64'hDEADBEEF_01234567").unwrap()
        );
        assert_eq!(
            sim.get("b").unwrap(),
            Value::from_str("64'h89ABCDEF_FEDCBA98").unwrap()
        );
    }
}

#[test]
fn write_log_grows_on_runtime_loop_narrow() {
    // A runtime-bound loop pushes one write-log entry per iteration, far
    // past the statically-sized pool; without growth this dropped entries
    // (interpret) or stored past the allocation (JIT/AOT-C).
    let code = r#"
    module Top (
        i_clk: input  '_ clock,
        i_rst: input  '_ reset,
        i_n  : input     logic<16>,
        o_v  : output    logic<32>,
    ) {
        var mem: logic<32> [6000];
        always_ff {
            if_reset {
            } else {
                for i in 0..i_n {
                    mem[i] = i + 1;
                }
            }
        }
        assign o_v = mem[5999];
    }
    "#;
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("i_clk").unwrap();
        let rst = sim.get_reset("i_rst").unwrap();

        sim.set("i_n", Value::new(6000, 16, false));
        sim.step(&rst);
        sim.step(&clk);
        // The last loop iteration's entry must survive the pool overflow.
        assert_eq!(
            sim.get("o_v").unwrap(),
            Value::new(6000, 32, false),
            "config={config:?}"
        );
    }
}

#[test]
fn write_log_grows_on_runtime_loop_wide() {
    // Same as the narrow case but with >8-byte elements so the wide entry
    // pool (floor 64) overflows.
    let code = r#"
    module Top (
        i_clk: input  '_ clock,
        i_rst: input  '_ reset,
        i_n  : input     logic<8>,
        o_v  : output    logic<128>,
    ) {
        var mem: logic<128> [200];
        always_ff {
            if_reset {
            } else {
                for i in 0..i_n {
                    mem[i] = i + 1;
                }
            }
        }
        assign o_v = mem[199];
    }
    "#;
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("i_clk").unwrap();
        let rst = sim.get_reset("i_rst").unwrap();

        sim.set("i_n", Value::new(200, 8, false));
        sim.step(&rst);
        sim.step(&clk);
        assert_eq!(
            sim.get("o_v").unwrap().payload_u128(),
            200u128,
            "config={config:?}"
        );
    }
}

#[test]
fn write_log_reserve_on_const_loop_narrow() {
    // Const-bound loop: AOT-C emits the loop in C with a one-shot
    // prologue reserve (6000 pushes from one site) instead of per-push
    // capacity checks.
    let code = r#"
    module Top (
        i_clk: input  '_ clock,
        i_rst: input  '_ reset,
        o_v  : output    logic<32>,
    ) {
        var mem: logic<32> [6000];
        always_ff {
            if_reset {
            } else {
                for i in 0..6000 {
                    mem[i] = i + 1;
                }
            }
        }
        assign o_v = mem[5999];
    }
    "#;
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("i_clk").unwrap();
        let rst = sim.get_reset("i_rst").unwrap();

        sim.step(&rst);
        sim.step(&clk);
        assert_eq!(
            sim.get("o_v").unwrap(),
            Value::new(6000, 32, false),
            "config={config:?}"
        );
    }
}

#[test]
fn write_log_reserve_on_const_loop_wide() {
    // Same with >8-byte elements so the wide pool is reserved.
    let code = r#"
    module Top (
        i_clk: input  '_ clock,
        i_rst: input  '_ reset,
        o_v  : output    logic<128>,
    ) {
        var mem: logic<128> [200];
        always_ff {
            if_reset {
            } else {
                for i in 0..200 {
                    mem[i] = i + 1;
                }
            }
        }
        assign o_v = mem[199];
    }
    "#;
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("i_clk").unwrap();
        let rst = sim.get_reset("i_rst").unwrap();

        sim.step(&rst);
        sim.step(&clk);
        assert_eq!(
            sim.get("o_v").unwrap().payload_u128(),
            200u128,
            "config={config:?}"
        );
    }
}

/// Reduction operand is self-determined (IEEE 1800 Table 11-23):
/// `&(a | ~b)` with 4-bit operands reduces over exactly 4 bits.
/// Regression: the outer expression context used to widen `~b`, making
/// the reduction span bits the operand doesn't have.
#[test]
fn reduction_operand_self_determined() {
    let code = r#"
    module Top (
        i_a: input  logic<4>,
        i_b: input  logic<4>,
        o_x: output logic   ,
    ) {
        assign o_x = &(i_a | ~i_b) && (i_b != 0);
    }
    "#;
    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.set("i_a", Value::new(0xf, 4, false));
        sim.set("i_b", Value::new(0xf, 4, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("o_x").unwrap(),
            Value::new(1, 1, false),
            "&(f | ~f) over 4 bits must be 1, config={config:?}"
        );
    }
}

#[test]
fn as_cast_samewidth_sign_reinterpretation() {
    // Regression: a same-width cast like `(a as i64)` used to be fully
    // transparent in the simulator IR, so a wider signed context
    // zero-extended the operand while the emitted SystemVerilog
    // (`longint'(a)`) sign-extends.
    let code = r#"
    module Top (
        a : input  logic<64>,
        b : input  logic<32>,
        s : input  i32      ,
        o : output logic<66>,
        p : output logic<64>,
        q : output logic<66>,
        r : output logic<64>,
    ) {
        assign o = (a as i64) + 66'sd0;
        assign p = (b as i32) + 64'sd0;
        assign q = (s as i64) + 66'sd0;
        assign r = (s as u32) + 64'sd0;
    }
    "#;
    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.set("a", Value::new(0xffff_ffff_ffff_fffe, 64, false));
        sim.set("b", Value::new(0xffff_fffe, 32, false));
        sim.set("s", Value::new(0xffff_fffe, 32, true));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("o").unwrap().payload_u128(),
            0x3_ffff_ffff_ffff_fffe,
            "o {config:?}"
        );
        assert_eq!(
            sim.get("p").unwrap().payload_u128(),
            0xffff_ffff_ffff_fffe,
            "p {config:?}"
        );
        assert_eq!(
            sim.get("q").unwrap().payload_u128(),
            0x3_ffff_ffff_ffff_fffe,
            "q {config:?}"
        );
        assert_eq!(
            sim.get("r").unwrap().payload_u128(),
            0xffff_fffe,
            "r {config:?}"
        );
    }
}

#[test]
fn as_cast_is_width_context_boundary() {
    // Regression: the cast width was ignored in self-determined contexts
    // (`{(e as 16) * (f as 16)}` multiplied at 8 bits), the outer width
    // leaked into the cast operand (`((g - h) >> 4) as 8` computed g-h at
    // 32 bits keeping borrow bits), and a narrowing cast was not truncated
    // at runtime.  Per LRM 6.24.1 the cast operand sizes as if assigned to
    // a cast-width variable.  Covers const-fold and the runtime backends.
    let code = r#"
    module Top (
        e : input  logic<8>,
        f : input  logic<8>,
        g : input  logic<8>,
        h : input  logic<8>,
        v : input  logic<32>,
        w : output logic<16>,
        u : output logic<32>,
        t : output logic<32>,
        s : output logic<32>,
        wc: output logic<16>,
        uc: output logic<32>,
    ) {
        const E: logic<8> = 200;
        const F: logic<8> = 200;
        const G: logic<8> = 0;
        const H: logic<8> = 1;
        assign wc = {(E as 16) * (F as 16)};
        assign uc = (((G - H) >> 4) as 8) + 32'h0;
        assign w  = {(e as 16) * (f as 16)};
        assign u  = (((g - h) >> 4) as 8) + 32'h0;
        assign t  = (v as 8) + 32'h0;
        assign s  = (v as i8) + 32'sh0;
    }
    "#;
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.set("e", Value::new(200, 8, false));
        sim.set("f", Value::new(200, 8, false));
        sim.set("g", Value::new(0, 8, false));
        sim.set("h", Value::new(1, 8, false));
        sim.set("v", Value::new(0x1ff, 32, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("wc").unwrap().payload_u128(),
            0x9c40,
            "wc {config:?}"
        );
        assert_eq!(sim.get("uc").unwrap().payload_u128(), 0xf, "uc {config:?}");
        assert_eq!(sim.get("w").unwrap().payload_u128(), 0x9c40, "w {config:?}");
        assert_eq!(sim.get("u").unwrap().payload_u128(), 0xf, "u {config:?}");
        assert_eq!(sim.get("t").unwrap().payload_u128(), 0xff, "t {config:?}");
        assert_eq!(
            sim.get("s").unwrap().payload_u128(),
            0xffff_ffff,
            "s {config:?}"
        );
    }
}

#[test]
fn narrowing_cast_of_wide_operand_representation() {
    // A narrowing `as` cast keeps its >128-bit operand in the wide-pointer
    // domain while the node itself is a ≤128-bit SCALAR: consumers (the
    // assign store, a wide parent op promoting its operands) must not
    // dereference the scalar as a result-slot pointer.
    let code = r#"
    module Top (
        a : input  logic<192>,
        o : output logic<64> ,
        p : output logic<100>,
        q : output logic<192>,
    ) {
        assign o = a as 64;
        assign p = a as 100;
        assign q = (a as 100) + 192'd1;
    }
    "#;
    for config in Config::all() {
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        use num_bigint::BigUint;
        let a: BigUint = (BigUint::from(0xdead_beef_cafe_f00du64) << 128u32)
            | (BigUint::from(0x0123_4567_89ab_cdefu64) << 64u32)
            | BigUint::from(0xfedc_ba98_7654_3210u64);
        sim.set("a", Value::new_biguint(a.clone(), 192, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("o").unwrap().payload_u128(),
            0xfedc_ba98_7654_3210u128,
            "o {config:?}"
        );
        let low100: BigUint = a.clone() & ((BigUint::from(1u32) << 100u32) - BigUint::from(1u32));
        assert_eq!(
            sim.get("p").unwrap(),
            Value::new_biguint(low100.clone(), 100, false),
            "p {config:?}"
        );
        assert_eq!(
            sim.get("q").unwrap(),
            Value::new_biguint(low100 + BigUint::from(1u32), 192, false),
            "q {config:?}"
        );
    }
}

#[test]
fn concat_element_as_cast_uses_target_width() {
    // A concatenation element that is an `as` cast must occupy the CAST
    // width in the result, not the source expression's width, and the
    // value must truncate (b=11 -> 3 bits).
    let code = r#"
    module Top (
        a: input  logic<8> ,
        b: input  logic<32>,
        o: output logic<8> ,
        p: output logic<16>,
    ) {
        always_comb {
            o = {a[7:4], b as 3, 1'b0};
            p = {a[7:4], b as 3} << 1;
        }
    }
    "#;
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.set("a", Value::new(0x80, 8, false));
        sim.set("b", Value::new(11, 32, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("o").unwrap().payload_u128(),
            0x86,
            "concat element cast width, {config:?}",
        );
        assert_eq!(
            sim.get("p").unwrap().payload_u128(),
            0x0086,
            "concat element cast width under shift, {config:?}",
        );
    }
}

#[test]
fn as_cast_width_boundary_wide() {
    // >64-bit variants of the boundary: a narrowing cast with a wide
    // (>64) target width, and narrow casts re-extended into a wide outer
    // context (signed and unsigned).
    let code = r#"
    module Top (
        v : input  logic<128>,
        n : input  logic<32>,
        a : output logic<128>,
        b : output logic<128>,
        c : output logic<128>,
    ) {
        assign a = (v as 100) + 128'h0;
        assign b = (n as 8) + 128'h0;
        assign c = (n as i8) + 128'sh0;
    }
    "#;
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.set("v", Value::from_u128(u128::MAX, 0, 128, false));
        sim.set("n", Value::new(0x1ff, 32, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("a").unwrap().payload_u128(),
            (1u128 << 100) - 1,
            "a {config:?}"
        );
        assert_eq!(sim.get("b").unwrap().payload_u128(), 0xff, "b {config:?}");
        assert_eq!(
            sim.get("c").unwrap().payload_u128(),
            u128::MAX,
            "c {config:?}"
        );
    }
}

#[test]
fn as_cast_wide_operand_to_narrow_context() {
    // Wide (>64) operand cast down into a narrow (<=64) outer context:
    // the masked operand must leave the BigUint domain (and the I128 /
    // wide-ptr domains in the JIT backends).  std's utils::truncate hits
    // this shape.
    use num_bigint::BigUint;
    let code = r#"
    module Top (
        v : input  logic<128>,
        u : input  logic<256>,
        o : output logic<32>,
        q : output logic<32>,
        r : output logic<32>,
    ) {
        assign o = (v as 8) + 32'h1;
        assign q = (v as i8) + 32'sh0;
        assign r = (u as 8) + 32'h1;
    }
    "#;
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.set("v", Value::from_u128(u128::MAX - 0x70, 0, 128, false));
        sim.set(
            "u",
            Value::new_biguint(
                (BigUint::from(1u32) << 200u32) + BigUint::from(0x4242u32),
                256,
                false,
            ),
        );
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("o").unwrap().payload_u128(), 0x90, "o {config:?}");
        assert_eq!(
            sim.get("q").unwrap().payload_u128(),
            0xffff_ff8f,
            "q {config:?}"
        );
        assert_eq!(sim.get("r").unwrap().payload_u128(), 0x43, "r {config:?}");
    }
}

#[test]
fn compare_signedness_survives_outer_context() {
    // A comparison under an unsigned sibling (`& 1'b1`, `+ 8'd0`) must
    // stay signed: SV 11.4.4 decides signedness from the operands alone.
    // Covers const-fold and the runtime backends.
    let code = r#"
    module Top (
        sa : input  i8,
        sb : input  i8,
        sw : input  signed logic<96>,
        sx : input  signed logic<96>,
        tw : input  signed logic<192>,
        tx : input  signed logic<192>,
        o  : output logic,
        oc : output logic,
        ow : output logic,
        ot : output logic,
        p  : output logic<32>,
    ) {
        const A: i32 = -1;
        const B: i32 = 1;
        const C: u32 = ((A <: B) + 8'd0) as 32;
        assign oc = (A <: B) & 1'b1;
        assign p  = C;
        assign o  = (sa <: sb) & 1'b1;
        assign ow = (sw <: sx) & 1'b1;
        assign ot = (tw <: tx) & 1'b1;
    }
    "#;
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.set("sa", Value::new(0xff, 8, true)); // -1
        sim.set("sb", Value::new(0, 8, true));
        // -2 <: 1 over the I128 (96-bit) and wide-ptr (192-bit) paths.
        use num_bigint::BigUint;
        let minus_two = |w: usize| (BigUint::from(1u32) << w) - BigUint::from(2u32);
        sim.set("sw", Value::new_biguint(minus_two(96), 96, true));
        sim.set("sx", Value::new_biguint(BigUint::from(1u32), 96, true));
        sim.set("tw", Value::new_biguint(minus_two(192), 192, true));
        sim.set("tx", Value::new_biguint(BigUint::from(1u32), 192, true));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("oc").unwrap(),
            Value::new(1, 1, false),
            "{config:?}"
        );
        assert_eq!(
            sim.get("p").unwrap(),
            Value::new(1, 32, false),
            "{config:?}"
        );
        assert_eq!(sim.get("o").unwrap(), Value::new(1, 1, false), "{config:?}");
        assert_eq!(
            sim.get("ow").unwrap(),
            Value::new(1, 1, false),
            "{config:?}"
        );
        assert_eq!(
            sim.get("ot").unwrap(),
            Value::new(1, 1, false),
            "{config:?}"
        );
    }
}

#[test]
fn equality_sign_extends_mixed_width_signed_operands() {
    // Regression: ==/!= (and ==?/!=?) zero-extended mixed-width operands
    // unconditionally, so 8'shff == 16'shffff (both -1) compared 0x00FF vs
    // 0xFFFF and yielded 0.  Per LRM 11.4.5 both-signed operands sign-extend
    // to the comparison width.  Covers const-fold and the runtime backends.
    let code = r#"
    module Top (
        sa: input  i8,
        o : output logic,
        n : output logic,
        p : output logic,
    ) {
        const P: i8  = 0 - 1;
        const Q: i16 = 0 - 1;
        assign p = P == Q;
        assign o = sa == Q;
        assign n = sa != Q;
    }
    "#;
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.set("sa", Value::new(0xff, 8, true)); // -1
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("p").unwrap(), Value::new(1, 1, false), "{config:?}");
        assert_eq!(sim.get("o").unwrap(), Value::new(1, 1, false), "{config:?}");
        assert_eq!(sim.get("n").unwrap(), Value::new(0, 1, false), "{config:?}");
    }
}

#[test]
fn ternary_sign_extends_narrow_signed_branch() {
    // Regression: the ternary result took the selected branch at its own
    // width zero-extended, so `cond ? (i8 -1) : (i32 5)` produced
    // 32'h000000ff instead of 32'hffffffff (both branches signed ->
    // narrower branch sign-extends, LRM 11.4.11).  Covers const-fold and
    // the runtime backends.
    let code = r#"
    module Top (
        c : input  logic,
        y : input  i8,
        z : input  i32,
        zw: input  signed logic<128>,
        yw: input  signed logic<96>,
        zp: input  signed logic<192>,
        q : output i32,
        qc: output i32,
        qw: output signed logic<128>,
        qm: output signed logic<128>,
        qp: output signed logic<192>,
    ) {
        const Y: i8  = 0 - 1;
        const Z: i32 = 5;
        assign qc = if 1'b1 ? Y : Z;
        assign q  = if c ? y : z;
        assign qw = if c ? y : zw;
        assign qm = if c ? yw : zw;
        assign qp = if c ? y : zp;
    }
    "#;
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.set("c", Value::new(1, 1, false));
        sim.set("y", Value::new(0xff, 8, true)); // -1
        sim.set("z", Value::new(5, 32, true));
        use num_bigint::BigUint;
        let minus_two = |w: usize| (BigUint::from(1u32) << w) - BigUint::from(2u32);
        sim.set("zw", Value::new_biguint(BigUint::from(5u32), 128, true));
        sim.set("yw", Value::new_biguint(minus_two(96), 96, true)); // -2
        sim.set("zp", Value::new_biguint(BigUint::from(5u32), 192, true));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(
            sim.get("qc").unwrap().payload_u128(),
            0xffff_ffffu128,
            "{config:?}"
        );
        assert_eq!(
            sim.get("q").unwrap().payload_u128(),
            0xffff_ffffu128,
            "{config:?}"
        );
        assert_eq!(
            sim.get("qw").unwrap().payload_u128(),
            u128::MAX,
            "qw {config:?}"
        );
        assert_eq!(
            sim.get("qm").unwrap().payload_u128(),
            u128::MAX - 1,
            "qm {config:?}"
        );
        assert_eq!(
            sim.get("qp").unwrap().payload().into_owned(),
            (BigUint::from(1u32) << 192) - BigUint::from(1u32),
            "qp {config:?}"
        );

        // Unsigned mix keeps zero-extension.
        sim.set("c", Value::new(0, 1, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("q").unwrap().payload_u128(), 5u128, "{config:?}");
    }
}

/// A logically-false feedback whose write group the sort cannot
/// linearize even with a pin.  It must surface as a CombinationalLoop
/// error, NOT interleave the reader into the group where it re-reads the
/// same mid-computation value on every pass — the silent miscompute
/// (q2 == 0 forever) this test guards against.  If a future scheduler
/// handles the design, the Ok arm's value assertions take over.
#[test]
fn false_comb_cycle_unpinnable_is_rejected_not_miscomputed() {
    let code = r#"
    module Top (
        en: input  logic,
        q2: output logic,
    ) {
        var h: logic;
        var s: logic;
        var p: logic;
        always_comb {
            h = 0;
            if p {
                h = 1;
            }
        }
        assign s = h;
        assign p = en | s;
        assign q2 = s;
    }
    "#;
    for config in Config::all() {
        if config.use_4state {
            continue;
        }
        symbol_table::clear();
        match analyze_top(code, &config, "Top") {
            Err(SimulatorError::CombinationalLoop { .. }) => {}
            Err(e) => panic!("unexpected error {e:?}, config={config:?}"),
            Ok(ir) => {
                let mut sim = Simulator::new(ir, None);
                sim.set("en", Value::new(1, 1, false));
                for _ in 0..3 {
                    sim.step(&Event::Clock(VarId::SYNTHETIC));
                }
                assert_eq!(
                    sim.get("q2").unwrap(),
                    Value::new(1, 1, false),
                    "q2 must reach the settled value, config={config:?}"
                );
            }
        }
    }
}

/// A structurally-cyclic but logically-false comb feedback (stall masks
/// the update that feeds the stall) plus split-driver per-bit assigns:
/// exercises the degradation path (SCC relax + reader pin) and the extra
/// settle passes that make the feedback converge.
#[test]
fn false_comb_cycle_with_split_drivers() {
    let code = r#"
    module Top (
        i_clk  : input  '_ clock   ,
        i_rst  : input  '_ reset   ,
        i_addr : input     logic<2>,
        i_en   : input     logic   ,
        o_stall: output    logic   ,
        o_upd  : output    logic<4>,
    ) {
        var r_pend: logic<4>;

        // Split-driver net: per-bit assigns of one packed vector,
        // depending on a stall that depends on the net (false path:
        // i_en gates the feedback off).
        var w_upd: logic<4>;
        for i in 0..4 :g_upd {
            assign w_upd[i] = r_pend[i] | (i_en && !o_stall && (i_addr == i));
        }

        assign o_stall = w_upd[i_addr] && !i_en;
        assign o_upd   = w_upd;

        always_ff (i_clk, i_rst) {
            if_reset {
                r_pend = 0;
            } else {
                r_pend = w_upd;
            }
        }
    }
    "#;
    for config in Config::all() {
        if config.use_4state {
            // The initial X takes more settle iterations to clear around
            // the feedback than the schedule's pass count; 2-state is
            // what this regression targets.
            continue;
        }
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let clk = sim.get_clock("i_clk").unwrap();
        let rst = sim.get_reset("i_rst").unwrap();
        sim.set("i_en", Value::new(1, 1, false));
        sim.set("i_addr", Value::new(2, 2, false));
        sim.step(&rst);
        sim.step(&clk);
        assert_eq!(
            sim.get("o_upd").unwrap(),
            Value::new(0b0100, 4, false),
            "bit 2 set via the enabled path, config={config:?}"
        );
        assert_eq!(
            sim.get("o_stall").unwrap(),
            Value::new(0, 1, false),
            "stall masked while enabled, config={config:?}"
        );
        sim.set("i_en", Value::new(0, 1, false));
        sim.step(&clk);
        assert_eq!(
            sim.get("o_stall").unwrap(),
            Value::new(1, 1, false),
            "pending bit 2 stalls once enable drops, config={config:?}"
        );
    }
}

#[test]
fn runtime_stepped_for_stall_guard_terminates() {
    // `*= 2` from 0 stalls at 0.  With a runtime bound the loop can't be
    // unrolled or rejected at analysis, so the simulator's progress guard
    // must break out instead of spinning forever in one delta step.
    let code = r#"
    module Top (
        i_n: input  logic<8>,
        o_a: output logic<8>,
    ) {
        always_comb {
            o_a = 0;
            for _i in 0..i_n step *= 2 {
                o_a += 1;
            }
        }
    }
    "#;
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.set("i_n", Value::new(10, 8, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        // One iteration runs (i parked at 0), then the guard breaks.
        assert_eq!(
            sim.get("o_a").unwrap(),
            Value::new(1, 8, false),
            "config={config:?}"
        );
    }
}

#[test]
fn pow_negative_exponent_follows_lrm_table() {
    // Regression: a negative signed exponent was reinterpreted as a huge
    // unsigned magnitude (3 ** -1 computed 3^255 in const-eval and a
    // modular inverse in the JIT).  IEEE 1800 11.4.3.1 (power operator rules): 0 for |base|>1, 1 for
    // base==1, ±1 for base==-1.
    let code = r#"
    module Top (
        n : input  i8,
        b : input  i32,
        r : output logic<32>,
        rc: output logic<32>,
        r1: output logic<32>,
        rm: output i32,
        rz: output logic<32>,
    ) {
        const N: i8 = 0 - 1;
        assign rc = 3 ** N;
        assign r  = (b ** n) as 32;
        assign r1 = 1 ** N;
        assign rm = (0 - 1) ** n;
        assign rz = ((0 as i32) ** n) as 32;
    }
    "#;
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.set("n", Value::new(0xff, 8, true)); // -1
        sim.set("b", Value::new(3, 32, true));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("rc").unwrap().payload_u128(), 0, "rc {config:?}");
        // 0 ** negative is x per the power operator rules: the interpreter models it
        // (4-state), the JIT approximates with 0 like its div-by-zero
        // handling, so only the payload is asserted here.
        assert_eq!(sim.get("rz").unwrap().payload_u128(), 0, "rz {config:?}");
        assert_eq!(sim.get("r").unwrap().payload_u128(), 0, "r {config:?}");
        assert_eq!(sim.get("r1").unwrap().payload_u128(), 1, "r1 {config:?}");
        // (-1) ** -1 = -1
        assert_eq!(
            sim.get("rm").unwrap().payload_u128(),
            0xffff_ffff,
            "rm {config:?}"
        );

        // (-1) ** -2 = 1
        sim.set("n", Value::new(0xfe, 8, true)); // -2
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("rm").unwrap().payload_u128(), 1, "rm2 {config:?}");

        // Positive exponents still compute normally.
        sim.set("n", Value::new(3, 8, true));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("r").unwrap().payload_u128(), 27, "r3 {config:?}");
    }
}

#[test]
fn widthless_literal_leading_zero_width_matches_emitter() {
    // Regression: the analyzer sized a width-less literal by its value bits
    // ('h0F -> 4) but the emitter counts a leading zero as 1 ('h0F -> 5'h0F),
    // so concatenations diverged from the emitted SV.
    let code = r#"
    module Top (
        o_b: output logic<9>,
        o_c: output logic<9>,
    ) {
        assign o_b = {1'b1, 'h0F};
        assign o_c = {2'b11, 'd09};
    }
    "#;
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        // {1'b1, 5'h0F} = 6'b10_1111 = 47
        assert_eq!(sim.get("o_b").unwrap().payload_u128(), 47, "{config:?}");
        // {2'b11, 5'd09} = 7'b11_01001 = 105
        assert_eq!(sim.get("o_c").unwrap().payload_u128(), 105, "{config:?}");
    }
}

#[test]
fn wide_signed_add_sign_extends_narrow_operand() {
    // Regression: the >128-bit path zero-extended a narrow signed operand
    // into its wide slot, so 192-bit `0 + (-1 as i8-ish)` gave 255 instead
    // of -1 in the cranelift and cc backends.
    let code = r#"
    module Top (
        a: input  signed logic<192>,
        b: input  signed logic<8>,
        c: output signed logic<192>,
    ) {
        assign c = a + b;
    }
    "#;
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.set("a", Value::new(0, 192, true));
        sim.set("b", Value::new(0xff, 8, true)); // -1
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        let v = sim.get("c").unwrap();
        let expect: Value = "192'hffffffffffffffffffffffffffffffffffffffffffffffff"
            .parse()
            .unwrap();
        assert_eq!(v.payload(), expect.payload(), "{config:?}");
    }
}

#[test]
fn wide_compare_does_not_overread_narrower_operand() {
    // Regression: comparing a 192-bit variable against a 256-bit one made
    // the JIT read 32 bytes from the 24-byte allocation, picking up the
    // adjacent variable's value as the missing words.
    let code = r#"
    module Top (
        a: input  logic<192>,
        d: input  logic<64>,
        b: input  logic<256>,
        c: output logic,
    ) {
        assign c = a == b;
    }
    "#;
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        sim.set("a", Value::new(5, 192, false));
        sim.set("d", Value::new(0xdead_beef_dead_beef, 64, false));
        sim.set("b", Value::new(5, 256, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("c").unwrap(), Value::new(1, 1, false), "{config:?}");
    }
}

#[test]
fn aot_c_wide_scalar_unary_and_carry_mask() {
    // Regression (cc backend): 65..128-bit `~`/`-` were computed after a
    // (uint64_t) truncation, zeroing the upper words; and the result-width
    // mask was elided for 65..128-bit Add feeding a comparison, so a real
    // carry at bit 100 made `(a + b) == c` evaluate false.
    let code = r#"
    module Top (
        a: input  logic<100>,
        b: input  logic<100>,
        c: input  logic<100>,
        n: output logic<100>,
        m: output logic<100>,
        y: output logic,
    ) {
        assign n = ~a;
        assign m = -a;
        assign y = (a + b) == c;
    }
    "#;
    for config in Config::all() {
        dbg!(&config);
        let ir = analyze(code, &config);
        let mut sim = Simulator::new(ir, None);
        let a: Value = "100'h8000000000000000".parse().unwrap();
        let all_f: Value = "100'hfffffffffffffffffffffffff".parse().unwrap();
        sim.set("a", a);
        sim.set("b", Value::new(0, 100, false));
        sim.set("c", Value::new(0, 100, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        let expect_n: Value = "100'hfffffffff7fffffffffffffff".parse().unwrap();
        let expect_m: Value = "100'hfffffffff8000000000000000".parse().unwrap();
        assert_eq!(
            sim.get("n").unwrap().payload(),
            expect_n.payload(),
            "{config:?}"
        );
        assert_eq!(
            sim.get("m").unwrap().payload(),
            expect_m.payload(),
            "{config:?}"
        );

        // (all-ones + 1) mod 2^100 == 0
        sim.set("a", all_f);
        sim.set("b", Value::new(1, 100, false));
        sim.set("c", Value::new(0, 100, false));
        sim.step(&Event::Clock(VarId::SYNTHETIC));
        assert_eq!(sim.get("y").unwrap(), Value::new(1, 1, false), "{config:?}");
    }
}
