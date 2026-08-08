//! Deferred whole-module AOT-C for the incremental (change-driven) settle.
//!
//! Under `VERYL_INCR=1` whole-module AOT-C is gated off at conv time: the
//! incremental engine replaces `Ir::settle_comb` outright, so a landed `.so`
//! would never be dispatched while its emit + background `cc` cost the whole
//! process (see `ProtoModule::conv`).  But two runtime outcomes put the
//! module back on the full-settle fallback permanently — the plan build
//! declining at instantiate time, and the runtime auto-abandon
//! (`IncrState::abandoned`) — and there the fallback is exactly the default
//! pipeline again, minus its whole-module backends.
//!
//! This bundle snapshots everything the conv-time compile would have used
//! (pre-JIT comb stmts, per-event pre-JIT stmts, localization info, config)
//! so the compile can start *late*, on a background thread, the moment one
//! of those outcomes materialises.  Landed handles are published through
//! `OnceLock` slots that `Ir::settle_comb` / `Simulator::eval_event_stmts`
//! poll; until then both stay on per-chunk Cranelift — the same code they
//! run while a conv-time async compile is pending.
//!
//! Dispatching the whole-event `.so` is safe here precisely because both
//! trigger conditions imply the settle-side seed/mark machinery is out of
//! the picture (no plan, or `abandoned` disables it): the `.so` being an
//! untracked writer for the dirty-seed lists — the reason whole-event is
//! gated under a live plan — no longer matters.

use super::CompiledWhole;
use crate::HashMap;
use crate::ir::{Config, Event, ProtoStatement};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// Comb offsets the AOT-C emitter must not localize + runtime-indexed array
/// ranges, precomputed in `ProtoModule::conv` while events and derived-clock
/// candidates are in scope (see `aot_c::emit::set_localize_blocklist`).
pub type LocalizeInfo = (crate::HashSet<isize>, Vec<(isize, usize, isize)>);

/// One event's pre-JIT statements + the slot its compiled handle lands in.
type EventSlot = (Vec<ProtoStatement>, OnceLock<Arc<dyn CompiledWhole>>);

/// Late-AOT-C opt-out (`VERYL_LATE_AOTC=0`); default on.
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("VERYL_LATE_AOTC").ok().as_deref() != Some("0"))
}

// ── Conv feedback ───────────────────────────────────────────────────────────
//
// A runtime abandon (or build-time plan decline) is a *deterministic verdict*
// on a comb list: the same DUT will reach the same permanent full-settle
// fallback in every later test of the suite, each one first paying the plan
// build, the pre-abandon bookkeeping window, and the unbatched event lists.
// `spawn` records the comb-pipeline key here; `ProtoModule::conv` consults it
// and takes the infeasible (default-pipeline) configuration from the start —
// batching, conv-time whole-module AOT-C, no plan.  The consult XORs
// `FLAVOR_RUNTIME_INFEASIBLE` into the key so the infeasible-flavor pipeline
// and `.so` cache entries never collide with the incremental-flavor ones
// already stored under the base key (the statement lists differ).

/// Folded into the comb-pipeline key when the conv-feedback flavor applies.
pub const FLAVOR_RUNTIME_INFEASIBLE: u128 = 0x1a7e_a07c_feed_bacc_9e37_79b9_7f4a_7c15;

static RUNTIME_INFEASIBLE: OnceLock<Mutex<crate::HashSet<u128>>> = OnceLock::new();

fn runtime_infeasible_set() -> &'static Mutex<crate::HashSet<u128>> {
    RUNTIME_INFEASIBLE.get_or_init(|| Mutex::new(crate::HashSet::default()))
}

/// Conv-feedback opt-out (`VERYL_INCR_CONV_FEEDBACK=0`); default on.
fn feedback_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("VERYL_INCR_CONV_FEEDBACK").ok().as_deref() != Some("0"))
}

/// Merge the persisted verdicts (one hex key per line) into the in-memory
/// set, once per process.  Unparsable lines are skipped, a missing file is
/// an empty set.  Keys are structural fingerprints of the comb list, so a
/// changed DUT or compiler version simply never matches — stale entries
/// are inert (delete the file, or `veryl clean`, to reset outright).
fn ensure_loaded(path: &Path, set: &mut crate::HashSet<u128>) {
    static LOADED: OnceLock<()> = OnceLock::new();
    LOADED.get_or_init(|| {
        if let Ok(text) = std::fs::read_to_string(path) {
            set.extend(
                text.lines()
                    .filter_map(|l| u128::from_str_radix(l.trim(), 16).ok()),
            );
        }
    });
}

fn note_runtime_infeasible(key: u128, path: Option<&Path>) {
    let mut set = runtime_infeasible_set().lock().unwrap();
    if let Some(path) = path {
        ensure_loaded(path, &mut set);
    }
    if set.insert(key)
        && let Some(path) = path
    {
        // Small O_APPEND writes from concurrent workers land whole; a rare
        // duplicate line is harmless (set semantics on load).  Errors are
        // ignored — persistence is an optimization, not a contract.
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
        {
            let _ = writeln!(f, "{key:032x}");
        }
    }
}

/// Whether a prior instance of this comb list abandoned (or was declined)
/// its incremental plan — in this process or a previous one (`path`) — so
/// conv should take the infeasible flavor.
pub fn runtime_infeasible(key: u128, path: Option<&Path>) -> bool {
    if !feedback_enabled() {
        return false;
    }
    let mut set = runtime_infeasible_set().lock().unwrap();
    if let Some(path) = path {
        ensure_loaded(path, &mut set);
    }
    set.contains(&key)
}

/// The one whole-module comb compile, shared by the conv-time path
/// (`ProtoModule::conv`) and the deferred one ([`LateAotc::compile`]).
///
/// The two must emit from identical inputs or the deferred `.so`
/// mis-simulates, so every emit input is a parameter here: adding one is a
/// compile error at both call sites until it is also threaded through
/// [`LateAotc::new`]'s snapshot.  Memoised on `key` so a suite's shared DUT
/// is emitted and fingerprinted once, whichever path gets there first.
pub fn compile_whole_comb(
    backends: &mut super::BackendRegistry,
    config: &Config,
    key: u128,
    dut_reuse: bool,
    stmts: &[ProtoStatement],
    #[cfg_attr(target_family = "wasm", allow(unused_variables))] localize: Option<&LocalizeInfo>,
) -> Option<Arc<dyn CompiledWhole>> {
    crate::ir::comb_pipeline_cache::whole_comb_get_or_compute(key, dut_reuse, || {
        let ctx = super::CompileCtx {
            config,
            use_4state: config.use_4state,
            contains_compiled_block: false,
        };
        // Hand the precomputed localization blocklist + array ranges to the
        // AOT-C emitter (no-op when VERYL_AOT_C_LOCALIZE is off), then clear
        // it so it can't leak into a later module's emit.  The blocklist is
        // thread-local, so the deferred path installs it on its own thread.
        #[cfg(not(target_family = "wasm"))]
        if let Some((block, ranges)) = localize {
            super::aot_c::emit::set_localize_blocklist(block.clone(), ranges.clone());
        }
        let r = backends.try_compile_whole_comb(&ctx, stmts);
        #[cfg(not(target_family = "wasm"))]
        super::aot_c::emit::clear_localize_blocklist();
        r
    })
}

#[cfg_attr(target_family = "wasm", allow(dead_code))]
pub struct LateAotc {
    /// Structural comb-pipeline key; memoises the whole-comb `.so` across
    /// tests sharing a DUT (same cache the conv-time compile uses).
    key: u128,
    dut_reuse: bool,
    config: Config,
    /// Post-sort post-DCE comb statements (shared with the pipeline cache).
    comb_stmts: Arc<Vec<ProtoStatement>>,
    localize: Option<LocalizeInfo>,
    /// Per-event pre-JIT statements + landing slot.  Empty when
    /// `Config::aot_c_event` is off.
    events: HashMap<Event, EventSlot>,
    started: AtomicBool,
    comb_slot: OnceLock<Arc<dyn CompiledWhole>>,
    /// Bumped once per landed event handle so the simulator's per-event
    /// dispatch cache knows to refresh.
    event_version: AtomicU32,
}

impl LateAotc {
    pub fn new(
        key: u128,
        dut_reuse: bool,
        config: Config,
        comb_stmts: Arc<Vec<ProtoStatement>>,
        localize: Option<LocalizeInfo>,
        event_stmts: HashMap<Event, Vec<ProtoStatement>>,
    ) -> Self {
        let events = event_stmts
            .into_iter()
            .map(|(e, s)| (e, (s, OnceLock::new())))
            .collect();
        LateAotc {
            key,
            dut_reuse,
            config,
            comb_stmts,
            localize,
            events,
            started: AtomicBool::new(false),
            comb_slot: OnceLock::new(),
            event_version: AtomicU32::new(0),
        }
    }

    /// Kick off the compile on a background thread.  Idempotent — only the
    /// first caller spawns; the handles land in the slots when ready.
    pub fn spawn(this: &Arc<Self>) {
        if this.started.swap(true, Ordering::SeqCst) {
            return;
        }
        // The verdict is deterministic for this comb list — feed it back so
        // later convs of the same DUT (this process and, when persistence is
        // configured, later ones) skip the incremental configuration.
        note_runtime_infeasible(this.key, this.config.incr_feedback_path.as_deref());
        #[cfg(not(target_family = "wasm"))]
        {
            let this = Arc::clone(this);
            let _ = std::thread::Builder::new()
                .name("late-aotc".into())
                .spawn(move || this.compile());
        }
    }

    #[cfg(not(target_family = "wasm"))]
    fn compile(&self) {
        use super::{BackendRegistry, CompileCtx};
        // NOTE: this mirrors the conv-time whole-module compile
        // (`ProtoModule::conv` -> `try_compile_whole_comb`/`_event`).  The two
        // snapshots must stay in lockstep: any new emit input (a localize
        // category, a CompileCtx field, a required pre-pass) added there must
        // be threaded into `LateAotc::new`'s snapshot and here, or the
        // deferred `.so` is emitted from an incomplete input and mis-simulates.
        let mut backends = BackendRegistry::for_config(&self.config);
        let whole = compile_whole_comb(
            &mut backends,
            &self.config,
            self.key,
            self.dut_reuse,
            &self.comb_stmts,
            self.localize.as_ref(),
        );
        let ctx = CompileCtx {
            config: &self.config,
            use_4state: self.config.use_4state,
            contains_compiled_block: false,
        };
        let comb_landed = whole.is_some();
        if let Some(w) = whole {
            let _ = self.comb_slot.set(w);
        }
        let mut events_landed = 0usize;
        for (event, (stmts, slot)) in &self.events {
            if let Some(w) = backends.try_compile_whole_event(&ctx, event, stmts)
                && slot.set(w).is_ok()
            {
                self.event_version.fetch_add(1, Ordering::Release);
                events_landed += 1;
            }
        }
        log::info!(
            "late whole-module AOT-C: comb={} events={}/{}",
            if comb_landed { "committed" } else { "declined" },
            events_landed,
            self.events.len(),
        );
    }

    /// Landed whole-comb handle, if any.  One atomic load; `None` while the
    /// compile is pending or after a decline.
    pub fn whole_comb(&self) -> Option<&Arc<dyn CompiledWhole>> {
        self.comb_slot.get()
    }

    /// Landed whole-event handle for `event`, if any.
    pub fn whole_event(&self, event: &Event) -> Option<&Arc<dyn CompiledWhole>> {
        self.events.get(event).and_then(|(_, slot)| slot.get())
    }

    /// Monotonic count of landed event handles; a change tells the
    /// simulator's per-event dispatch cache to refresh.
    pub fn event_version(&self) -> u32 {
        self.event_version.load(Ordering::Acquire)
    }
}
