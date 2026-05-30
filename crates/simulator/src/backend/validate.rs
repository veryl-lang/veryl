//! `VERYL_AOT_C_VALIDATE` dual-run validation.
//!
//! When enabled, dispatches AOT-C (whole-comb) and Cranelift (per-chunk)
//! on identical inputs, diffs ff/comb buffers + write_log, panics on
//! divergence.  Cranelift's per-chunk dispatch is not currently a
//! `CompiledWhole`, so this module takes `&Ir` / `&dyn CompiledWhole`
//! directly rather than composing two backends.

use crate::backend::{CompiledWhole, DispatchOutcome};
use crate::ir::{Ir, ModuleVariables};
use crate::simulator::SimProfile;
use veryl_analyzer::value::MaskCache;

/// Dispatch `whole` on a snapshot, restore inputs, run Cranelift, diff
/// results, panic on divergence.  On success the buffers reflect the
/// Cranelift output (the ground truth).  `NotReady` from `whole` skips
/// validation and runs Cranelift only.
pub fn settle_comb(
    ir: &Ir,
    whole: &dyn CompiledWhole,
    passes: usize,
    mask_cache: &mut MaskCache,
    profile: &mut SimProfile,
) {
    let ff_ptr = ir.ff_values.as_ptr();
    let comb_ptr = ir.comb_values.as_ptr() as *mut u8;
    let log_ptr = (&*ir.write_log_buffer as *const _ as *const u8) as *mut u8;

    // Snapshot inputs so we can restore them for the Cranelift run.
    let ff_snap_in: Vec<u8> = ir.ff_values.to_vec();
    let comb_snap_in: Vec<u8> = ir.comb_values.to_vec();
    let count_snap_in: u64 = ir.write_log_buffer.count() as u64;

    for _ in 0..passes {
        match whole.try_dispatch(ff_ptr, comb_ptr, log_ptr) {
            DispatchOutcome::Done => {}
            DispatchOutcome::NotReady => {
                ir.run_chunked_settle(mask_cache, profile);
                return;
            }
        }
    }

    let ff_aot_out: Vec<u8> = ir.ff_values.to_vec();
    let comb_aot_out: Vec<u8> = ir.comb_values.to_vec();
    let count_aot_out: u64 = ir.write_log_buffer.count() as u64;

    // Restore inputs so Cranelift starts from the same state.
    unsafe {
        let ff_dst = ir.ff_values.as_ptr() as *mut u8;
        std::ptr::copy_nonoverlapping(ff_snap_in.as_ptr(), ff_dst, ff_snap_in.len());
        let comb_dst = ir.comb_values.as_ptr() as *mut u8;
        std::ptr::copy_nonoverlapping(comb_snap_in.as_ptr(), comb_dst, comb_snap_in.len());
    }
    let _ = count_snap_in;

    ir.run_chunked_settle(mask_cache, profile);

    let ff_jit_out: &[u8] = &ir.ff_values;
    let comb_jit_out: &[u8] = &ir.comb_values;
    let count_jit_out: u64 = ir.write_log_buffer.count() as u64;

    diff_or_panic(
        ir,
        comb_ptr,
        &comb_snap_in,
        &comb_aot_out,
        comb_jit_out,
        &ff_aot_out,
        ff_jit_out,
        count_aot_out,
        count_jit_out,
    );
}

#[allow(clippy::too_many_arguments)]
fn diff_or_panic(
    ir: &Ir,
    comb_ptr: *mut u8,
    comb_snap_in: &[u8],
    comb_aot_out: &[u8],
    comb_jit_out: &[u8],
    ff_aot_out: &[u8],
    ff_jit_out: &[u8],
    count_aot_out: u64,
    count_jit_out: u64,
) {
    let mut diverged = false;
    if comb_aot_out != comb_jit_out {
        let off = comb_aot_out
            .iter()
            .zip(comb_jit_out.iter())
            .position(|(a, b)| a != b)
            .unwrap_or(usize::MAX);
        let var_info = lookup_comb_offset(&ir.module_variables, comb_ptr, off);
        let dump_word = |snap: &[u8], byte_off: isize, name: &str| {
            let abs = (off as isize + byte_off) as usize;
            if abs + 4 <= snap.len() {
                let w = u32::from_le_bytes(snap[abs..abs + 4].try_into().unwrap_or([0; 4]));
                eprintln!("  snap[{:+}] ({}) = 0x{:08x} (u32)", byte_off, name, w);
            }
        };
        eprintln!(
            "VERYL_AOT_C_VALIDATE: comb_values diverge at offset {} \
             (AOT-C={:#x}, JIT={:#x}, len={}) var={}",
            off,
            comb_aot_out.get(off).copied().unwrap_or(0),
            comb_jit_out.get(off).copied().unwrap_or(0),
            comb_aot_out.len(),
            var_info,
        );
        eprintln!("  input snapshot (relative to diverge byte):");
        for delta in (-64..=64).step_by(4) {
            dump_word(comb_snap_in, delta as isize, "comb");
        }
        eprintln!("  AOT-C output around diverge byte:");
        for delta in (-64..=64).step_by(4) {
            dump_word(comb_aot_out, delta as isize, "aot");
        }
        eprintln!("  JIT output around diverge byte:");
        for delta in (-64..=64).step_by(4) {
            let w = comb_jit_out
                .get(((off as isize + delta) as usize)..((off as isize + delta + 4) as usize))
                .map(|s| u32::from_le_bytes(s.try_into().unwrap_or([0; 4])))
                .unwrap_or(0);
            eprintln!("  out[{:+}] (jit) = 0x{:08x} (u32)", delta, w);
        }
        eprintln!("  ALL diverging byte ranges:");
        let mut run_start: Option<usize> = None;
        let mut count = 0usize;
        let max_runs = 32usize;
        let pairs: Vec<(usize, u8, u8)> = comb_aot_out
            .iter()
            .zip(comb_jit_out.iter())
            .enumerate()
            .filter_map(|(i, (a, b))| if a != b { Some((i, *a, *b)) } else { None })
            .collect();
        for (i, &(idx, _, _)) in pairs.iter().enumerate() {
            let is_contig = run_start.is_some() && i > 0 && pairs[i - 1].0 + 1 == idx;
            if !is_contig {
                if let Some(start) = run_start {
                    let end = pairs[i - 1].0;
                    let info = lookup_comb_offset(&ir.module_variables, comb_ptr, start);
                    eprintln!(
                        "    [{}-{}] ({}B) at var={}",
                        start,
                        end,
                        end - start + 1,
                        info,
                    );
                    count += 1;
                    if count >= max_runs {
                        eprintln!("    ... ({} more diverging bytes total)", pairs.len() - i);
                        break;
                    }
                }
                run_start = Some(idx);
            }
        }
        if let Some(start) = run_start
            && count < max_runs
            && let Some(&(end, _, _)) = pairs.last()
        {
            let info = lookup_comb_offset(&ir.module_variables, comb_ptr, start);
            eprintln!(
                "    [{}-{}] ({}B) at var={}",
                start,
                end,
                end - start + 1,
                info,
            );
        }
        diverged = true;
    }
    if ff_aot_out != ff_jit_out {
        let off = ff_aot_out
            .iter()
            .zip(ff_jit_out.iter())
            .position(|(a, b)| a != b)
            .unwrap_or(usize::MAX);
        eprintln!(
            "VERYL_AOT_C_VALIDATE: ff_values diverge at offset {} \
             (AOT-C={:#x}, JIT={:#x}, len={})",
            off,
            ff_aot_out.get(off).copied().unwrap_or(0),
            ff_jit_out.get(off).copied().unwrap_or(0),
            ff_aot_out.len(),
        );
        diverged = true;
    }
    if count_aot_out != count_jit_out {
        eprintln!(
            "VERYL_AOT_C_VALIDATE: write_log count diverges (AOT-C={}, JIT={})",
            count_aot_out, count_jit_out,
        );
        diverged = true;
    }
    if diverged {
        panic!("AOT-C / Cranelift divergence in settle_comb");
    }
}

/// Map a byte offset back to a variable name via hierarchy walk.
/// Used by the diff output to annotate diverging bytes.
pub fn lookup_comb_offset(vars: &ModuleVariables, comb_base: *const u8, target: usize) -> String {
    fn walk(
        vars: &ModuleVariables,
        target_addr: usize,
        cover: &mut Vec<String>,
        nearby: &mut Vec<(isize, String)>,
    ) {
        for var in vars.variables.values() {
            for (i, &ptr) in var.current_values.iter().enumerate() {
                let addr = ptr as usize;
                let end = addr + var.native_bytes;
                if (target_addr >= addr) && (target_addr < end) {
                    cover.push(format!(
                        "{}[{}]+{} (w={}, nb={})",
                        var.path,
                        i,
                        target_addr - addr,
                        var.width,
                        var.native_bytes,
                    ));
                }
                let delta = addr as isize - target_addr as isize;
                if delta.unsigned_abs() <= 64 {
                    nearby.push((
                        delta,
                        format!(
                            "{}[{}]@{:+} (w={}, nb={})",
                            var.path, i, delta, var.width, var.native_bytes,
                        ),
                    ));
                }
            }
        }
        for child in &vars.children {
            walk(child, target_addr, cover, nearby);
        }
    }
    let target_addr = comb_base as usize + target;
    let mut cover: Vec<String> = Vec::new();
    let mut nearby: Vec<(isize, String)> = Vec::new();
    walk(vars, target_addr, &mut cover, &mut nearby);
    if cover.is_empty() && nearby.is_empty() {
        return "?".to_string();
    }
    nearby.sort_by_key(|(d, _)| d.abs());
    let primary = cover.first().cloned().unwrap_or_else(|| "?".to_string());
    let cover_n = cover.len();
    let cover_extra = if cover_n > 1 {
        format!(
            " [+{} other covers: {}]",
            cover_n - 1,
            cover[1..].join("; ")
        )
    } else {
        String::new()
    };
    let nearby_str: Vec<String> = nearby.iter().take(16).map(|(_, s)| s.clone()).collect();
    format!(
        "{primary}{cover_extra} | nearby: [{}]",
        nearby_str.join(", "),
    )
}
