use crate::ir::{
    Event, Ir, ModuleVariables, Statement, Value, VarId, VarPath, dispatch_stmt_fast,
    read_native_value, write_native_value,
};
use crate::wave_dumper::{DumpVar, WaveDumper};
use std::str::FromStr;
use veryl_analyzer::value::MaskCache;

#[cfg(feature = "profile")]
#[derive(Default, Debug)]
pub struct SimProfile {
    pub step_count: u64,
    pub settle_comb_count: u64,
    pub comb_eval_count: u64,
    pub extra_pass_count: u64,
    pub converged_first_try: u64,
    pub settle_comb_ns: u64,
    pub event_eval_ns: u64,
    pub ff_swap_ns: u64,
    pub eval_comb_full_ns: u64,
}

#[cfg(not(feature = "profile"))]
#[derive(Default, Debug)]
pub struct SimProfile;

pub struct Simulator {
    pub ir: Ir,
    pub time: u64,
    pub dump: Option<WaveDumper>,
    dump_vars: Vec<DumpVar>,
    pub mask_cache: MaskCache,
    comb_dirty: bool,
    /// Scratch buffer used only by the worklist (`eval_comb_worklist`)
    /// evaluation path; the default settle_comb path does not touch it.
    comb_snapshot_buf: Vec<u8>,
    pub profile: SimProfile,
    last_event: Option<Event>,
    last_event_stmts: *const Vec<Statement>,
}

impl Simulator {
    pub fn new(ir: Ir, dump: Option<WaveDumper>) -> Self {
        let comb_snapshot_buf = vec![0u8; ir.comb_values.len()];
        let mut ret = Self {
            ir,
            time: 0,
            dump: None,
            dump_vars: Vec::new(),
            mask_cache: MaskCache::default(),
            comb_dirty: true,
            comb_snapshot_buf,
            profile: Default::default(),
            last_event: None,
            last_event_stmts: std::ptr::null(),
        };

        if let Some(dumper) = dump {
            ret.setup_dump(dumper);
        }

        ret
    }

    fn do_settle_comb(&mut self) {
        self.ir.settle_comb(
            &mut self.mask_cache,
            &mut self.comb_snapshot_buf,
            &mut self.profile,
        );
    }

    pub fn set(&mut self, port: &str, value: Value) {
        let port = VarPath::from_str(port).unwrap();

        if let Some(id) = self.ir.ports.get(&port)
            && let Some(x) = self.ir.module_variables.variables.get_mut(id)
        {
            let mut value = value;
            value.trunc(x.width);
            unsafe {
                write_native_value(
                    x.current_values[0],
                    x.native_bytes,
                    self.ir.use_4state,
                    &value,
                );
            }
            self.comb_dirty = true;
        }
    }

    pub fn get(&mut self, port: &str) -> Option<Value> {
        self.ensure_comb_updated();

        let port = VarPath::from_str(port).unwrap();

        if let Some(id) = self.ir.ports.get(&port)
            && let Some(x) = self.ir.module_variables.variables.get(id)
        {
            let value = unsafe {
                read_native_value(
                    x.current_values[0],
                    x.native_bytes,
                    self.ir.use_4state,
                    x.width as u32,
                    false,
                )
            };
            Some(value)
        } else {
            None
        }
    }

    /// Get a variable value by hierarchical path (e.g., "dut.cnt").
    /// Searches all module variables including children.
    pub fn get_var(&mut self, path: &str) -> Option<Value> {
        self.ensure_comb_updated();

        let target = VarPath::from_str(path).unwrap();
        Self::find_var_in_module(&self.ir.module_variables, &target, self.ir.use_4state)
    }

    fn find_var_in_module(
        module: &ModuleVariables,
        target: &VarPath,
        use_4state: bool,
    ) -> Option<Value> {
        // If target has multiple segments, try matching child module by name first
        if target.0.len() > 1 {
            for child in &module.children {
                if child.name == target.0[0] {
                    let sub = VarPath::from_slice(&target.0[1..]);
                    if let Some(v) = Self::find_var_in_module(child, &sub, use_4state) {
                        return Some(v);
                    }
                }
            }
        }

        // Look for a variable whose path matches exactly
        for var in module.variables.values() {
            if var.path == *target {
                let value = unsafe {
                    read_native_value(
                        var.current_values[0],
                        var.native_bytes,
                        use_4state,
                        var.width as u32,
                        false,
                    )
                };
                return Some(value);
            }
        }
        None
    }

    pub fn ensure_comb_updated(&mut self) {
        if self.comb_dirty {
            #[cfg(feature = "profile")]
            let start = std::time::Instant::now();

            self.do_settle_comb();
            self.comb_dirty = false;

            #[cfg(feature = "profile")]
            {
                self.profile.settle_comb_ns += start.elapsed().as_nanos() as u64;
            }
        }
    }

    pub fn mark_comb_dirty(&mut self) {
        self.comb_dirty = true;
    }

    pub fn get_clock(&self, port: &str) -> Option<Event> {
        let port = VarPath::from_str(port).unwrap();
        self.ir.ports.get(&port).map(|id| Event::Clock(*id))
    }

    pub fn get_reset(&self, port: &str) -> Option<Event> {
        let port = VarPath::from_str(port).unwrap();
        self.ir.ports.get(&port).map(|id| Event::Reset(*id))
    }

    pub fn step(&mut self, event: &Event) {
        #[cfg(feature = "profile")]
        {
            self.profile.step_count += 1;
        }

        if self.comb_dirty {
            #[cfg(feature = "profile")]
            let start = std::time::Instant::now();

            self.do_settle_comb();
            self.comb_dirty = false;

            #[cfg(feature = "profile")]
            {
                self.profile.settle_comb_ns += start.elapsed().as_nanos() as u64;
            }
        }

        #[cfg(feature = "profile")]
        let event_start = std::time::Instant::now();

        let stmts_ptr = if self.last_event.as_ref() == Some(event) {
            self.last_event_stmts
        } else {
            let ptr: *const Vec<Statement> = match self.ir.event_statements.get(event) {
                Some(v) => v as *const _,
                None => std::ptr::null(),
            };
            self.last_event = Some(event.clone());
            self.last_event_stmts = ptr;
            ptr
        };
        if !stmts_ptr.is_null() {
            // SAFETY: event_statements is never mutated after Ir construction.
            let statements: &Vec<Statement> = unsafe { &*stmts_ptr };
            for x in statements {
                dispatch_stmt_fast(x, &mut self.mask_cache);
            }
        }

        #[cfg(feature = "profile")]
        {
            self.profile.event_eval_ns += event_start.elapsed().as_nanos() as u64;
        }

        #[cfg(feature = "profile")]
        let ff_start = std::time::Instant::now();

        Self::ff_commit_specialized(
            &mut self.ir.ff_values,
            &self.ir.ff_commit_u32_runs,
            &self.ir.ff_commit_u64_runs,
            &self.ir.ff_commit_other,
            self.ir.ff_commit_use_avx2,
        );

        #[cfg(feature = "profile")]
        {
            self.profile.ff_swap_ns += ff_start.elapsed().as_nanos() as u64;
        }

        self.comb_dirty = true;

        self.dump_variables();
    }

    /// Commit FF updates: copy next → current for all FF variables.
    /// Dispatches to a vpermd/vpermq AVX2 shuffle on x86_64 when the
    /// CPU supports it, falling back to a scalar loop otherwise.
    #[inline(always)]
    fn ff_commit_specialized(
        ff_values: &mut [u8],
        u32_runs: &[(u32, u32)],
        u64_runs: &[(u32, u32)],
        other: &[(usize, usize)],
        use_avx2: bool,
    ) {
        let ptr = ff_values.as_mut_ptr();
        #[cfg(target_arch = "x86_64")]
        {
            if use_avx2 {
                unsafe { Self::ff_commit_runs_avx2(ptr, u32_runs, u64_runs) };
            } else {
                Self::ff_commit_runs_scalar(ptr, u32_runs, u64_runs);
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            let _ = use_avx2;
            Self::ff_commit_runs_scalar(ptr, u32_runs, u64_runs);
        }
        for &(current_offset, value_size) in other {
            unsafe {
                let dst = ptr.add(current_offset);
                let src = ptr.add(current_offset + value_size);
                std::ptr::copy_nonoverlapping(src, dst, value_size);
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2")]
    unsafe fn ff_commit_runs_avx2(ptr: *mut u8, u32_runs: &[(u32, u32)], u64_runs: &[(u32, u32)]) {
        unsafe {
            use std::arch::x86_64::*;
            // u32: 4 entries per 32-byte chunk.  [c0|n0|c1|n1|c2|n2|c3|n3]
            // vpermd mask [1,1,3,3,5,5,7,7] → [n0|n0|n1|n1|n2|n2|n3|n3].
            let mask = _mm256_setr_epi32(1, 1, 3, 3, 5, 5, 7, 7);
            for &(start, count) in u32_runs {
                let base = ptr.add(start as usize);
                let count = count as usize;
                let chunks = count / 4;
                for i in 0..chunks {
                    let p = base.add(i * 32) as *mut __m256i;
                    let v = _mm256_loadu_si256(p);
                    let shuffled = _mm256_permutevar8x32_epi32(v, mask);
                    _mm256_storeu_si256(p, shuffled);
                }
                for i in (chunks * 4)..count {
                    let dst = base.add(i * 8) as *mut u32;
                    let src = base.add(i * 8 + 4) as *const u32;
                    let s = std::ptr::read_unaligned(src);
                    std::ptr::write_unaligned(dst, s);
                }
            }
            // u64: 2 entries per 32-byte chunk.  [c0|n0|c1|n1] (4 u64 lanes)
            // vpermq imm8 [1,1,3,3] → [n0|n0|n1|n1].
            for &(start, count) in u64_runs {
                let base = ptr.add(start as usize);
                let count = count as usize;
                let chunks = count / 2;
                for i in 0..chunks {
                    let p = base.add(i * 32) as *mut __m256i;
                    let v = _mm256_loadu_si256(p);
                    let shuffled = _mm256_permute4x64_epi64::<0b11_11_01_01>(v);
                    _mm256_storeu_si256(p, shuffled);
                }
                for i in (chunks * 2)..count {
                    let dst = base.add(i * 16) as *mut u64;
                    let src = base.add(i * 16 + 8) as *const u64;
                    let s = std::ptr::read_unaligned(src);
                    std::ptr::write_unaligned(dst, s);
                }
            }
        }
    }

    #[inline]
    fn ff_commit_runs_scalar(ptr: *mut u8, u32_runs: &[(u32, u32)], u64_runs: &[(u32, u32)]) {
        for &(start, count) in u32_runs {
            unsafe {
                let base = ptr.add(start as usize);
                for i in 0..count as usize {
                    let dst = base.add(i * 8) as *mut u32;
                    let src = base.add(i * 8 + 4) as *const u32;
                    let s = std::ptr::read_unaligned(src);
                    std::ptr::write_unaligned(dst, s);
                }
            }
        }
        for &(start, count) in u64_runs {
            unsafe {
                let base = ptr.add(start as usize);
                for i in 0..count as usize {
                    let dst = base.add(i * 16) as *mut u64;
                    let src = base.add(i * 16 + 8) as *const u64;
                    let s = std::ptr::read_unaligned(src);
                    std::ptr::write_unaligned(dst, s);
                }
            }
        }
    }

    /// Set a variable value by VarId. Used to write clock/reset signal values
    /// into the variable storage so they appear in wave dumps.
    pub fn set_var_by_id(&mut self, var_id: &VarId, val: Value) {
        if let Some(x) = self.ir.module_variables.variables.get_mut(var_id) {
            let mut val = val;
            val.trunc(x.width);
            unsafe {
                write_native_value(
                    x.current_values[0],
                    x.native_bytes,
                    self.ir.use_4state,
                    &val,
                );
            }
            self.comb_dirty = true;
        }
    }

    pub fn dump_start(&mut self) {
        if let Some(dump) = &mut self.dump {
            dump.begin_dumpvars();
            dump.dump_all_vars(&self.dump_vars, self.ir.use_4state);
            dump.end_dumpvars();
        }
    }

    pub fn dump_variables(&mut self) {
        if self.dump.is_some() {
            if self.comb_dirty {
                self.do_settle_comb();
                self.comb_dirty = false;
            }
            let dump = self.dump.as_mut().unwrap();
            dump.timestamp(self.time);
            dump.dump_all_vars(&self.dump_vars, self.ir.use_4state);
        }
    }

    fn setup_dump(&mut self, mut dumper: WaveDumper) {
        dumper.timescale();
        dumper.setup_module(&self.ir.module_variables, &mut self.dump_vars);
        dumper.finish_header();
        self.dump = Some(dumper);
    }
}
