//! Per-Ir FF write log buffer.
//!
//! NBA semantics: each FF write is recorded as a log entry and applied
//! to `current` storage at cycle end by `ff_commit_from_log`.  This
//! decouples the commit cost from total FF count (favorable in sparse-
//! write regimes).
//!
//! Two entry pools:
//! - **narrow** (16 B): payload ≤ 8 bytes (width_class ∈ {1, 2, 4, 8}).
//!   Covers the common case of byte-/halfword-/word-/dword-FFs.
//! - **wide** (64 B = 1 cache line): payload up to 56 bytes (≤ 448-bit
//!   FFs in one entry).  Wider FFs split into multiple wide entries.

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct WriteLogEntry {
    /// FF current byte offset within ff_values where the payload should be applied.
    pub offset: u32,
    /// X/Z mask payload, used only when use_4state is true; zero otherwise.
    pub mask_xz: u16,
    /// Width class tag: 1, 2, 4, 8 for u8/u16/u32/u64.
    pub width_class: u16,
    /// Stored value.  For widths < 8 bytes the upper bits are zero or
    /// undefined; the consumer truncates by width_class.
    pub payload: u64,
}

/// The byte range `(first, count)` a static bit field touches: a slice write
/// only changes those bytes, so logging the whole element spends `nb` payload
/// bytes to deposit a couple of bits.
pub(crate) fn static_field_byte_span(hi: usize, lo: usize, nb: usize) -> Option<(usize, usize)> {
    if hi < lo {
        return None;
    }
    let blo = lo / 8;
    let bhi = hi / 8;
    if bhi >= nb {
        return None;
    }
    Some((blo, bhi - blo + 1))
}

/// Wide-FF log entry.  64 bytes = 1 cache line, with up to 56 bytes of
/// payload (covers 64–448-bit FFs in a single entry; wider FFs use
/// multiple entries).  `align(64)` ensures each entry occupies exactly
/// one cache line so payload stores never straddle two lines.
#[repr(C, align(64))]
#[derive(Debug, Clone, Copy)]
pub struct WriteLogWideEntry {
    /// FF current byte offset within ff_values where the payload should be applied.
    pub offset: u32,
    /// Number of bytes from `payload` to copy.  Always ≤ 56.
    pub native_bytes: u8,
    pub _pad: [u8; 3],
    pub payload: [u8; 56],
}

impl Default for WriteLogWideEntry {
    fn default() -> Self {
        Self {
            offset: 0,
            native_bytes: 0,
            _pad: [0; 3],
            payload: [0; 56],
        }
    }
}

/// Per-Ir write log buffer with separate pools for narrow and wide FFs.
///
/// `#[repr(C)]` guarantees the field order and offsets so JIT-emitted code
/// can read/write `narrow_entries_ptr` / `narrow_count` and `wide_entries_ptr`
/// / `wide_count` at stable offsets.  `_owner` fields keep the heap
/// allocations alive; the `*_ptr` fields are cached at construction.
#[repr(C)]
#[derive(Debug)]
pub struct WriteLogBuffer {
    /// Narrow path: pointer to the [`WriteLogEntry`] array (16 B per entry).
    pub narrow_entries_ptr: *mut WriteLogEntry,
    /// Live narrow-entry count for this cycle.
    pub narrow_count: u32,
    /// Narrow capacity (doubles on overflow via `grow_narrow`).
    pub narrow_capacity: u32,
    /// Wide path: pointer to the [`WriteLogWideEntry`] array (64 B per entry).
    pub wide_entries_ptr: *mut WriteLogWideEntry,
    /// Live wide-entry count for this cycle.
    pub wide_count: u32,
    /// Wide capacity (doubles on overflow via `grow_wide`).
    pub wide_capacity: u32,
    /// Grow-and-push slow path for a full narrow pool, reached as a
    /// fn-pointer field (not a baked address) so disk-cached AOT-C objects
    /// stay valid across processes/ASLR.
    pub grow_push_narrow: unsafe extern "C" fn(*mut WriteLogBuffer, u32, u64, u32),
    /// Grow-and-push slow path for a full wide pool.
    pub grow_push_wide: unsafe extern "C" fn(*mut WriteLogBuffer, u32, *const u8, u32),
    /// Bulk-reserve slow path: ensures room for the given numbers of
    /// additional narrow/wide entries.  Called once from the AOT-C event
    /// prologue so the per-push code stays unchecked.
    pub reserve: unsafe extern "C" fn(*mut WriteLogBuffer, u32, u32),
    /// Owning storage — keeps `narrow_entries_ptr` valid.
    _narrow_owner: Box<[WriteLogEntry]>,
    /// Owning storage — keeps `wide_entries_ptr` valid.
    _wide_owner: Box<[WriteLogWideEntry]>,
}

// SAFETY: WriteLogBuffer is owned by a single Ir which is bound to a
// single thread (Ir is Send but not Sync).
unsafe impl Send for WriteLogBuffer {}

/// Layout offsets / sizes used by JIT-emitted inline write-log push code
/// (Cranelift `emit_inline_write_log_*` and cc/AOT-C `emit_log_push`).
/// Computed from the actual `#[repr(C)]` layout via `offset_of!` so a
/// field reorder propagates to the codegen automatically, and so the
/// constants stay correct on every target (pointer-sized fields differ
/// between 64-bit native and wasm32).  `allow(dead_code)` because
/// none of the codegen sites are reachable on wasm.
#[allow(dead_code)]
pub const WRITE_LOG_NARROW_OFFSET_ENTRIES_PTR: i32 =
    std::mem::offset_of!(WriteLogBuffer, narrow_entries_ptr) as i32;
#[allow(dead_code)]
pub const WRITE_LOG_NARROW_OFFSET_COUNT: i32 =
    std::mem::offset_of!(WriteLogBuffer, narrow_count) as i32;
#[allow(dead_code)]
pub const WRITE_LOG_NARROW_OFFSET_CAPACITY: i32 =
    std::mem::offset_of!(WriteLogBuffer, narrow_capacity) as i32;
#[allow(dead_code)]
pub const WRITE_LOG_WIDE_OFFSET_ENTRIES_PTR: i32 =
    std::mem::offset_of!(WriteLogBuffer, wide_entries_ptr) as i32;
#[allow(dead_code)]
pub const WRITE_LOG_WIDE_OFFSET_COUNT: i32 =
    std::mem::offset_of!(WriteLogBuffer, wide_count) as i32;
#[allow(dead_code)]
pub const WRITE_LOG_WIDE_OFFSET_CAPACITY: i32 =
    std::mem::offset_of!(WriteLogBuffer, wide_capacity) as i32;
#[allow(dead_code)]
pub const WRITE_LOG_OFFSET_GROW_PUSH_NARROW: i32 =
    std::mem::offset_of!(WriteLogBuffer, grow_push_narrow) as i32;
#[allow(dead_code)]
pub const WRITE_LOG_OFFSET_GROW_PUSH_WIDE: i32 =
    std::mem::offset_of!(WriteLogBuffer, grow_push_wide) as i32;
#[allow(dead_code)]
pub const WRITE_LOG_OFFSET_RESERVE: i32 = std::mem::offset_of!(WriteLogBuffer, reserve) as i32;

#[allow(dead_code)]
pub const WRITE_LOG_ENTRY_SIZE: i32 = std::mem::size_of::<WriteLogEntry>() as i32;
#[allow(dead_code)]
pub const WRITE_LOG_ENTRY_OFFSET_OFFSET: i32 = std::mem::offset_of!(WriteLogEntry, offset) as i32;
#[allow(dead_code)]
pub const WRITE_LOG_ENTRY_OFFSET_MASK_XZ: i32 = std::mem::offset_of!(WriteLogEntry, mask_xz) as i32;
#[allow(dead_code)]
pub const WRITE_LOG_ENTRY_OFFSET_WIDTH_CLASS: i32 =
    std::mem::offset_of!(WriteLogEntry, width_class) as i32;
#[allow(dead_code)]
pub const WRITE_LOG_ENTRY_OFFSET_PAYLOAD: i32 = std::mem::offset_of!(WriteLogEntry, payload) as i32;

#[allow(dead_code)]
pub const WRITE_LOG_WIDE_ENTRY_SIZE: i32 = std::mem::size_of::<WriteLogWideEntry>() as i32;
#[allow(dead_code)]
pub const WRITE_LOG_WIDE_ENTRY_OFFSET_OFFSET: i32 =
    std::mem::offset_of!(WriteLogWideEntry, offset) as i32;
#[allow(dead_code)]
pub const WRITE_LOG_WIDE_ENTRY_OFFSET_NB: i32 =
    std::mem::offset_of!(WriteLogWideEntry, native_bytes) as i32;
#[allow(dead_code)]
pub const WRITE_LOG_WIDE_ENTRY_OFFSET_PAYLOAD: i32 =
    std::mem::offset_of!(WriteLogWideEntry, payload) as i32;
/// Maximum payload bytes a single wide entry can hold.
pub const WRITE_LOG_WIDE_ENTRY_PAYLOAD_BYTES: usize = 56;

impl Default for WriteLogBuffer {
    fn default() -> Self {
        Self::with_capacity(0, 0)
    }
}

impl WriteLogBuffer {
    /// Allocate a buffer with `narrow_cap` narrow entries and `wide_cap` wide
    /// entries, all zero-initialized.
    pub fn with_capacity(narrow_cap: usize, wide_cap: usize) -> Self {
        let mut narrow = vec![WriteLogEntry::default(); narrow_cap].into_boxed_slice();
        let narrow_entries_ptr = narrow.as_mut_ptr();
        let mut wide = vec![WriteLogWideEntry::default(); wide_cap].into_boxed_slice();
        let wide_entries_ptr = wide.as_mut_ptr();
        Self {
            narrow_entries_ptr,
            narrow_count: 0,
            narrow_capacity: narrow_cap as u32,
            wide_entries_ptr,
            wide_count: 0,
            wide_capacity: wide_cap as u32,
            grow_push_narrow: write_log_grow_push_narrow,
            grow_push_wide: write_log_grow_push_wide,
            reserve: write_log_reserve,
            _narrow_owner: narrow,
            _wide_owner: wide,
        }
    }

    /// Grow the narrow pool to at least `min_cap` (next power of two,
    /// floor 4096), preserving live entries.  The buffer header address
    /// stays stable (Ir owns the buffer in a Box), so JIT/AOT-C inline
    /// pushes pick up the new pointer/capacity on their next load.
    #[cold]
    fn grow_narrow_to(&mut self, min_cap: usize) {
        let new_cap = min_cap.next_power_of_two().max(4096);
        let mut narrow = vec![WriteLogEntry::default(); new_cap].into_boxed_slice();
        let live = self.narrow_count as usize;
        narrow[..live].copy_from_slice(&self._narrow_owner[..live]);
        self.narrow_entries_ptr = narrow.as_mut_ptr();
        self.narrow_capacity = new_cap as u32;
        self._narrow_owner = narrow;
    }

    /// Grow the wide pool to at least `min_cap` (next power of two,
    /// floor 64), preserving live entries.
    #[cold]
    fn grow_wide_to(&mut self, min_cap: usize) {
        let new_cap = min_cap.next_power_of_two().max(64);
        let mut wide = vec![WriteLogWideEntry::default(); new_cap].into_boxed_slice();
        let live = self.wide_count as usize;
        wide[..live].copy_from_slice(&self._wide_owner[..live]);
        self.wide_entries_ptr = wide.as_mut_ptr();
        self.wide_capacity = new_cap as u32;
        self._wide_owner = wide;
    }

    /// Ensure room for `extra` more narrow entries.
    fn reserve_narrow(&mut self, extra: u32) {
        let needed = self.narrow_count as u64 + extra as u64;
        if needed > self.narrow_capacity as u64 {
            self.grow_narrow_to(needed as usize);
        }
    }

    /// Ensure room for `extra` more wide entries.
    fn reserve_wide(&mut self, extra: u32) {
        let needed = self.wide_count as u64 + extra as u64;
        if needed > self.wide_capacity as u64 {
            self.grow_wide_to(needed as usize);
        }
    }

    /// Append a narrow entry, growing the pool when full.
    fn push_narrow(&mut self, offset: u32, payload: u64, width_class: u16) {
        if self.narrow_count >= self.narrow_capacity {
            self.grow_narrow_to(self.narrow_capacity as usize + 1);
        }
        let idx = self.narrow_count as usize;
        // SAFETY: idx < narrow_capacity after the grow check above.
        unsafe {
            *self.narrow_entries_ptr.add(idx) = WriteLogEntry {
                offset,
                mask_xz: 0,
                width_class,
                payload,
            };
        }
        self.narrow_count += 1;
    }

    /// Append a wide entry, growing the pool when full.
    ///
    /// Safety: `payload` must be valid for reads of `native_bytes` (≤ 56) bytes.
    unsafe fn push_wide(&mut self, offset: u32, payload: *const u8, native_bytes: usize) {
        if self.wide_count >= self.wide_capacity {
            self.grow_wide_to(self.wide_capacity as usize + 1);
        }
        let idx = self.wide_count as usize;
        let entry = WriteLogWideEntry {
            offset,
            native_bytes: native_bytes as u8,
            _pad: [0; 3],
            payload: {
                let mut p = [0u8; WRITE_LOG_WIDE_ENTRY_PAYLOAD_BYTES];
                unsafe {
                    std::ptr::copy_nonoverlapping(payload, p.as_mut_ptr(), native_bytes);
                }
                p
            },
        };
        // SAFETY: idx < wide_capacity after the grow check above.
        unsafe {
            *self.wide_entries_ptr.add(idx) = entry;
        }
        self.wide_count += 1;
    }

    pub fn narrow_capacity(&self) -> usize {
        self.narrow_capacity as usize
    }

    pub fn wide_capacity(&self) -> usize {
        self.wide_capacity as usize
    }

    pub fn narrow_count(&self) -> u32 {
        self.narrow_count
    }

    pub fn wide_count(&self) -> u32 {
        self.wide_count
    }

    pub fn is_empty(&self) -> bool {
        self.narrow_count == 0 && self.wide_count == 0
    }

    pub fn reset(&mut self) {
        self.narrow_count = 0;
        self.wide_count = 0;
    }

    /// Total entries written this cycle (narrow + wide).  Used by diagnostics.
    pub fn count(&self) -> u32 {
        self.narrow_count + self.wide_count
    }

    /// Safe slice view of live narrow entries.
    pub fn narrow_entries_slice(&self) -> &[WriteLogEntry] {
        // SAFETY: narrow_entries_ptr points to a Box<[WriteLogEntry]> of length
        // narrow_capacity; narrow_count <= narrow_capacity by construction.
        unsafe {
            std::slice::from_raw_parts(self.narrow_entries_ptr, self.narrow_capacity as usize)
        }
    }

    /// Safe slice view of live wide entries.
    pub fn wide_entries_slice(&self) -> &[WriteLogWideEntry] {
        // SAFETY: wide_entries_ptr points to a Box<[WriteLogWideEntry]> of
        // length wide_capacity; wide_count <= wide_capacity by construction.
        unsafe { std::slice::from_raw_parts(self.wide_entries_ptr, self.wide_capacity as usize) }
    }
}

/// Apply each log entry's payload to the FF current slot.  Narrow entries are
/// applied first, then wide entries.  Within each pool, entries are processed
/// in insertion order so multiple writes to the same offset apply
/// last-write-wins, matching JIT/interpret semantics.
///
/// One body serves both public entry points (the settle filter alternates
/// between them at runtime on `comb_dirty`), monomorphized so the plain
/// commit pays nothing for the compare the watched one splices in — a drift
/// between two hand-kept copies would be a silent filter-on vs filter-off
/// divergence.
#[inline(always)]
fn commit_from_log_impl<const WATCHED: bool>(
    ff_values: &mut [u8],
    buffer: &WriteLogBuffer,
    mut watched: impl FnMut(usize, usize) -> bool,
) -> bool {
    let len = ff_values.len();
    let dst = ff_values.as_mut_ptr();
    let mut hit = false;

    // Narrow path: single word store per width class.
    let narrow_limit = buffer.narrow_count as usize;
    for entry in buffer.narrow_entries_slice().iter().take(narrow_limit) {
        let nb = entry.width_class as usize;
        let offset = entry.offset as usize;
        if offset + nb > len {
            continue;
        }
        // SAFETY: bounds verified above; dst is the start of the slice.
        unsafe {
            let p = dst.add(offset);
            match nb {
                8 => {
                    if WATCHED && !hit && (p as *const u64).read_unaligned() != entry.payload {
                        hit = watched(offset, nb);
                    }
                    (p as *mut u64).write_unaligned(entry.payload);
                }
                4 => {
                    if WATCHED && !hit && (p as *const u32).read_unaligned() != entry.payload as u32
                    {
                        hit = watched(offset, nb);
                    }
                    (p as *mut u32).write_unaligned(entry.payload as u32);
                }
                2 => {
                    if WATCHED && !hit && (p as *const u16).read_unaligned() != entry.payload as u16
                    {
                        hit = watched(offset, nb);
                    }
                    (p as *mut u16).write_unaligned(entry.payload as u16);
                }
                1 => {
                    if WATCHED && !hit && *p != entry.payload as u8 {
                        hit = watched(offset, nb);
                    }
                    *p = entry.payload as u8;
                }
                _ => {}
            }
        }
    }

    // Wide path.  Most entries are field-sized now, where `copy_from_slice` on
    // a runtime length is a `memcpy` call.  Pool, order and record are
    // unchanged, so same-byte writes still compose last-write-wins.
    let wide_limit = buffer.wide_count as usize;
    for entry in buffer.wide_entries_slice().iter().take(wide_limit) {
        let nb = entry.native_bytes as usize;
        let offset = entry.offset as usize;
        if nb == 0 || nb > WRITE_LOG_WIDE_ENTRY_PAYLOAD_BYTES || offset + nb > len {
            continue;
        }
        // SAFETY: bounds verified above; dst is the start of the slice, and
        // `payload` holds at least `nb` bytes since nb <= PAYLOAD_BYTES.  Every
        // access below lies inside [0, nb): the single-store arms match nb
        // exactly, and each pair arm is guarded by nb >= W so `nb - W` cannot
        // wrap and `nb - W + W == nb` is the last byte touched.
        unsafe {
            let p = dst.add(offset);
            let s = entry.payload.as_ptr();
            macro_rules! store_one {
                ($t:ty) => {{
                    let v = (s as *const $t).read_unaligned();
                    if WATCHED && !hit && (p as *const $t).read_unaligned() != v {
                        hit = watched(offset, nb);
                    }
                    (p as *mut $t).write_unaligned(v);
                }};
            }
            macro_rules! store_pair {
                ($t:ty, $w:expr) => {{
                    let tail = nb - $w;
                    let lo = (s as *const $t).read_unaligned();
                    let hi_v = (s.add(tail) as *const $t).read_unaligned();
                    if WATCHED
                        && !hit
                        && ((p as *const $t).read_unaligned() != lo
                            || (p.add(tail) as *const $t).read_unaligned() != hi_v)
                    {
                        hit = watched(offset, nb);
                    }
                    (p as *mut $t).write_unaligned(lo);
                    (p.add(tail) as *mut $t).write_unaligned(hi_v);
                }};
            }
            match nb {
                1 => {
                    let v = *s;
                    if WATCHED && !hit && *p != v {
                        hit = watched(offset, nb);
                    }
                    *p = v;
                }
                2 => store_one!(u16),
                4 => store_one!(u32),
                8 => store_one!(u64),
                3 => store_pair!(u16, 2),
                5..=7 => store_pair!(u32, 4),
                9..=16 => store_pair!(u64, 8),
                _ => {
                    // Through `dst` like the arms above: a `&mut ff_values`
                    // reborrow here would pop its tag, so the next iteration's
                    // store through it is UB under Stacked Borrows.
                    let p = dst.add(offset);
                    let s = entry.payload.as_ptr();
                    if WATCHED
                        && !hit
                        && std::slice::from_raw_parts(p, nb) != std::slice::from_raw_parts(s, nb)
                    {
                        hit = watched(offset, nb);
                    }
                    std::ptr::copy_nonoverlapping(s, p, nb);
                }
            }
        }
    }
    hit
}

/// See [`commit_from_log_impl`].
pub fn ff_commit_from_log(ff_values: &mut [u8], buffer: &WriteLogBuffer) {
    commit_from_log_impl::<false>(ff_values, buffer, |_, _| false);
}

/// [`commit_from_log_impl`] with a change probe for the settle filter: each
/// entry's payload is compared against the bytes it overwrites, and the
/// return value says whether any byte that actually CHANGED lies where
/// `watched` answers true.  After the first watched change the remaining
/// entries commit without comparing.
///
/// A write that changes bytes and a later same-cycle write that restores
/// them still reports a watched change — a false positive that only costs
/// one settle, never a missed one.
pub fn ff_commit_from_log_watched(
    ff_values: &mut [u8],
    buffer: &WriteLogBuffer,
    watched: &mut dyn FnMut(usize, usize) -> bool,
) -> bool {
    commit_from_log_impl::<true>(ff_values, buffer, watched)
}

use std::cell::Cell;
use std::ptr::NonNull;

thread_local! {
    /// Per-thread pointer to the active `WriteLogBuffer`.  `Simulator::step`
    /// installs the current Ir's buffer before invoking event JIT/interpret,
    /// then clears it after `ff_commit_from_log` finishes.  JIT-emitted FF
    /// writes and interpret-path FF writes call into this module's helpers
    /// to push entries.
    ///
    /// The pointer is `Option<NonNull>` (not raw `*mut`) so that
    /// `with_active(|buf| ...)` can avoid an aliasing-violation hazard:
    /// while a helper holds a `&mut` to the buffer no other helper can
    /// observe one.  Re-entrancy is impossible because helpers always run
    /// to completion before returning to JIT code that emitted them.
    static EVENT_WRITE_LOG: Cell<Option<NonNull<WriteLogBuffer>>> = const { Cell::new(None) };
}

/// Install `buf` as the active write log for this thread.  Must be paired
/// with `clear_event_write_log` once the cycle's emit phase ends.
///
/// Safety: caller must ensure `buf` remains valid until `clear` is called
/// and that no other concurrent thread is using the same buffer.
pub(crate) unsafe fn set_event_write_log(buf: &mut WriteLogBuffer) {
    EVENT_WRITE_LOG.with(|cell| {
        cell.set(Some(NonNull::from(buf)));
    });
}

pub(crate) fn clear_event_write_log() {
    EVENT_WRITE_LOG.with(|cell| cell.set(None));
}

/// Push a narrow FF write entry into the active log buffer.  Called from
/// JIT code (`extern "C"`) and from the interpret path.  Width class is
/// one of 1/2/4/8 (== native bytes).  Grows the pool when full — a write
/// site inside a runtime-bound loop can push any number of entries per
/// cycle, so the statically-sized pool is only a starting capacity.
///
/// Safety: caller is the JIT-emitted code which only invokes this while
/// the TLS is installed by `set_event_write_log`.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn event_write_log_push_static(
    offset: u32,
    payload: u64,
    width_class: u16,
) {
    EVENT_WRITE_LOG.with(|cell| {
        let Some(ptr) = cell.get() else {
            // No active log: emit becomes a no-op.  Reached when the
            // helper symbol is bound but the TLS hasn't been installed
            // (e.g., during initial-block paths before the write-log
            // is wired up).
            return;
        };
        let buf = unsafe { &mut *ptr.as_ptr() };
        buf.push_narrow(offset, payload, width_class);
    });
}

/// Push a wide FF write entry (used by the interpret path).  `payload`
/// must point to `native_bytes` (≤ 56) bytes of FF data.  Grows the pool
/// when full.
///
/// Safety: caller must ensure `payload` is valid for reads of
/// `native_bytes` bytes; the helper is only invoked while the TLS is
/// installed.
pub(crate) unsafe fn event_write_log_push_wide(
    offset: u32,
    payload: *const u8,
    native_bytes: usize,
) {
    debug_assert!(
        native_bytes <= WRITE_LOG_WIDE_ENTRY_PAYLOAD_BYTES,
        "wide payload {} exceeds entry capacity",
        native_bytes
    );
    EVENT_WRITE_LOG.with(|cell| {
        let Some(ptr) = cell.get() else {
            return;
        };
        let buf = unsafe { &mut *ptr.as_ptr() };
        unsafe {
            buf.push_wide(offset, payload, native_bytes);
        }
    });
}

/// Slow path for the JIT/AOT-C inline narrow push: grow the pool and
/// append.  Reached via the `grow_push_narrow` header field when the
/// pool is full.
///
/// Safety: `buf` must point to the live `WriteLogBuffer` whose header was
/// handed to the emitted code.
pub(crate) unsafe extern "C" fn write_log_grow_push_narrow(
    buf: *mut WriteLogBuffer,
    offset: u32,
    payload: u64,
    width_class: u32,
) {
    let buf = unsafe { &mut *buf };
    buf.push_narrow(offset, payload, width_class as u16);
}

/// Slow path for the JIT/AOT-C inline wide push: grow the pool and append.
///
/// Safety: `buf` must point to the live `WriteLogBuffer`; `payload` must be
/// valid for reads of `native_bytes` (≤ 56) bytes.
pub(crate) unsafe extern "C" fn write_log_grow_push_wide(
    buf: *mut WriteLogBuffer,
    offset: u32,
    payload: *const u8,
    native_bytes: u32,
) {
    let buf = unsafe { &mut *buf };
    unsafe {
        buf.push_wide(offset, payload, native_bytes as usize);
    }
}

/// Bulk-reserve entry point, reached via the `reserve` header field from
/// the AOT-C event prologue: one call per eval guarantees room for every
/// (unchecked) inline push in the body.
///
/// Safety: `buf` must point to the live `WriteLogBuffer` whose header was
/// handed to the emitted code.
pub(crate) unsafe extern "C" fn write_log_reserve(
    buf: *mut WriteLogBuffer,
    narrow: u32,
    wide: u32,
) {
    let buf = unsafe { &mut *buf };
    buf.reserve_narrow(narrow);
    buf.reserve_wide(wide);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_layout_is_16_bytes() {
        assert_eq!(core::mem::size_of::<WriteLogEntry>(), 16);
    }

    #[test]
    fn wide_entry_is_64_bytes() {
        assert_eq!(core::mem::size_of::<WriteLogWideEntry>(), 64);
    }

    #[test]
    fn empty_buffer_has_zero_count() {
        let b = WriteLogBuffer::with_capacity(0, 0);
        assert_eq!(b.narrow_capacity(), 0);
        assert_eq!(b.wide_capacity(), 0);
        assert_eq!(b.count(), 0);
        assert!(b.is_empty());
    }

    #[test]
    fn capacity_allocated() {
        let b = WriteLogBuffer::with_capacity(16, 4);
        assert_eq!(b.narrow_capacity(), 16);
        assert_eq!(b.wide_capacity(), 4);
        assert_eq!(b.count(), 0);
    }

    #[test]
    fn reset_clears_counts() {
        let mut b = WriteLogBuffer::with_capacity(4, 2);
        b.narrow_count = 3;
        b.wide_count = 1;
        b.reset();
        assert_eq!(b.narrow_count, 0);
        assert_eq!(b.wide_count, 0);
    }

    #[test]
    fn narrow_push_helper_writes_into_active_buffer() {
        let mut buf = WriteLogBuffer::with_capacity(4, 0);
        unsafe {
            set_event_write_log(&mut buf);
            event_write_log_push_static(0x1000, 0xdead_beef, 8);
            event_write_log_push_static(0x1008, 0xfeed_face, 4);
            clear_event_write_log();
        }
        assert_eq!(buf.narrow_count, 2);
        let entries = buf.narrow_entries_slice();
        assert_eq!(entries[0].offset, 0x1000);
        assert_eq!(entries[0].payload, 0xdead_beef);
        assert_eq!(entries[0].width_class, 8);
        assert_eq!(entries[1].offset, 0x1008);
        assert_eq!(entries[1].payload, 0xfeed_face);
        assert_eq!(entries[1].width_class, 4);
    }

    #[test]
    fn wide_push_helper_writes_into_active_buffer() {
        let mut buf = WriteLogBuffer::with_capacity(0, 2);
        let payload = [0xaau8; 32];
        unsafe {
            set_event_write_log(&mut buf);
            event_write_log_push_wide(0x2000, payload.as_ptr(), 32);
            clear_event_write_log();
        }
        assert_eq!(buf.wide_count, 1);
        let entries = buf.wide_entries_slice();
        assert_eq!(entries[0].offset, 0x2000);
        assert_eq!(entries[0].native_bytes, 32);
        assert_eq!(&entries[0].payload[..32], &payload[..]);
    }

    /// A typed store wider than `native_bytes` would pass a payload-only check
    /// and corrupt the neighbouring FF, so both borders are asserted.
    #[test]
    fn wide_commit_writes_exactly_native_bytes_for_every_size() {
        for nb in 1..=WRITE_LOG_WIDE_ENTRY_PAYLOAD_BYTES {
            let mut buf = WriteLogBuffer::with_capacity(0, 1);
            let payload: Vec<u8> = (0..nb).map(|i| 0x40 + i as u8).collect();
            unsafe { write_log_grow_push_wide(&mut buf, 8, payload.as_ptr(), nb as u32) };

            let mut ff = vec![0xcc_u8; 8 + nb + 8];
            ff_commit_from_log(&mut ff, &buf);

            assert_eq!(&ff[..8], &[0xcc; 8], "nb={nb} wrote before the offset");
            assert_eq!(&ff[8..8 + nb], &payload[..], "nb={nb} payload mismatch");
            assert_eq!(&ff[8 + nb..], &[0xcc; 8], "nb={nb} wrote past native_bytes");
        }
    }

    /// Same pool, same order: a later wide entry overwrites an earlier one
    /// byte for byte regardless of which store shape each one takes.  This is
    /// the property that bars routing by payload size, so it is pinned across
    /// the specialised/unspecialised boundary rather than within one arm.
    #[test]
    fn wide_commit_mixed_sizes_compose_last_write_wins() {
        let mut buf = WriteLogBuffer::with_capacity(0, 4);
        unsafe {
            write_log_grow_push_wide(&mut buf, 0, [0x11u8; 12].as_ptr(), 12); // memcpy arm
            write_log_grow_push_wide(&mut buf, 0, [0x22u8; 8].as_ptr(), 8); // typed
            write_log_grow_push_wide(&mut buf, 2, [0x33u8; 4].as_ptr(), 4); // typed
            write_log_grow_push_wide(&mut buf, 3, [0x44u8; 1].as_ptr(), 1); // typed
        }
        let mut ff = vec![0u8; 16];
        ff_commit_from_log(&mut ff, &buf);
        assert_eq!(
            &ff[..12],
            &[
                0x22, 0x22, 0x33, 0x44, 0x33, 0x33, 0x22, 0x22, 0x11, 0x11, 0x11, 0x11
            ]
        );
    }

    /// The watched commit's compare is typed on the specialised sizes and a
    /// slice compare on the rest.  A compare that looked at fewer bytes than
    /// it stores would silently under-report changes to the settle filter, so
    /// every size is probed on its LAST byte.
    #[test]
    fn wide_watched_compare_sees_the_last_byte_at_every_size() {
        for nb in 1..=WRITE_LOG_WIDE_ENTRY_PAYLOAD_BYTES {
            let payload = vec![0x5a_u8; nb];
            let mut buf = WriteLogBuffer::with_capacity(0, 1);
            unsafe { write_log_grow_push_wide(&mut buf, 0, payload.as_ptr(), nb as u32) };

            let mut same = payload.clone();
            let hit = ff_commit_from_log_watched(&mut same, &buf, &mut |_, _| true);
            assert!(!hit, "nb={nb} reported a change against identical bytes");

            let mut differs = payload.clone();
            differs[nb - 1] ^= 0xff;
            let hit = ff_commit_from_log_watched(&mut differs, &buf, &mut |_, _| true);
            assert!(hit, "nb={nb} missed a change in the last byte");
            assert_eq!(differs, payload, "nb={nb} did not commit the payload");
        }
    }

    #[test]
    fn narrow_push_grows_past_capacity() {
        let mut buf = WriteLogBuffer::with_capacity(4, 0);
        unsafe {
            set_event_write_log(&mut buf);
            for i in 0..5000u32 {
                event_write_log_push_static(i * 8, i as u64, 8);
            }
            clear_event_write_log();
        }
        assert_eq!(buf.narrow_count, 5000);
        assert!(buf.narrow_capacity() >= 5000);
        let entries = buf.narrow_entries_slice();
        for (i, entry) in entries.iter().enumerate().take(5000) {
            assert_eq!(entry.offset, i as u32 * 8);
            assert_eq!(entry.payload, i as u64);
        }
    }

    #[test]
    fn wide_push_grows_past_capacity() {
        let mut buf = WriteLogBuffer::with_capacity(0, 2);
        unsafe {
            set_event_write_log(&mut buf);
            for i in 0..70u32 {
                let payload = [i as u8; 16];
                event_write_log_push_wide(i * 16, payload.as_ptr(), 16);
            }
            clear_event_write_log();
        }
        assert_eq!(buf.wide_count, 70);
        assert!(buf.wide_capacity() >= 70);
        let entries = buf.wide_entries_slice();
        for (i, entry) in entries.iter().enumerate().take(70) {
            assert_eq!(entry.offset, i as u32 * 16);
            assert_eq!(entry.native_bytes, 16);
            assert_eq!(&entry.payload[..16], &[i as u8; 16]);
        }
    }

    #[test]
    fn reserve_grows_both_pools_preserving_entries() {
        let mut buf = WriteLogBuffer::with_capacity(4, 2);
        unsafe {
            write_log_grow_push_narrow(&mut buf, 0, 7, 8);
            write_log_reserve(&mut buf, 5000, 70);
        }
        assert!(buf.narrow_capacity() >= 5001);
        assert!(buf.wide_capacity() >= 70);
        assert_eq!(buf.narrow_count, 1);
        assert_eq!(buf.narrow_entries_slice()[0].payload, 7);
        // Within capacity: a no-op.
        let cap = buf.narrow_capacity();
        unsafe {
            write_log_reserve(&mut buf, 1, 1);
        }
        assert_eq!(buf.narrow_capacity(), cap);
    }

    #[test]
    fn grow_push_entry_points_append_when_full() {
        let mut buf = WriteLogBuffer::with_capacity(1, 1);
        unsafe {
            write_log_grow_push_narrow(&mut buf, 0, 1, 8);
            write_log_grow_push_narrow(&mut buf, 8, 2, 8);
            let payload = [0xa5u8; 16];
            write_log_grow_push_wide(&mut buf, 0, payload.as_ptr(), 16);
            write_log_grow_push_wide(&mut buf, 16, payload.as_ptr(), 16);
        }
        assert_eq!(buf.narrow_count, 2);
        assert_eq!(buf.wide_count, 2);
        assert_eq!(buf.narrow_entries_slice()[1].payload, 2);
        assert_eq!(buf.wide_entries_slice()[1].offset, 16);
    }

    #[test]
    fn watched_commit_reports_only_watched_changes() {
        // Watch the first 16 bytes.
        let watched = |off: usize, _len: usize| off < 16;

        // Same-value write: committed, not reported.
        let mut buf = WriteLogBuffer::with_capacity(4, 2);
        let mut ff = vec![0u8; 64];
        unsafe { write_log_grow_push_narrow(&mut buf, 0, 0, 8) };
        assert!(!ff_commit_from_log_watched(&mut ff, &buf, &mut { watched }));

        // Changed but unwatched: committed, not reported.
        let mut buf = WriteLogBuffer::with_capacity(4, 2);
        unsafe { write_log_grow_push_narrow(&mut buf, 32, 0x1234, 4) };
        assert!(!ff_commit_from_log_watched(&mut ff, &buf, &mut { watched }));
        assert_eq!(ff[32], 0x34);

        // Changed and watched (narrow): reported.
        let mut buf = WriteLogBuffer::with_capacity(4, 2);
        unsafe { write_log_grow_push_narrow(&mut buf, 8, 0xff, 1) };
        assert!(ff_commit_from_log_watched(&mut ff, &buf, &mut { watched }));
        assert_eq!(ff[8], 0xff);

        // Wide entry: unchanged then changed.
        let payload = [0xa5u8; 16];
        let mut buf = WriteLogBuffer::with_capacity(4, 2);
        unsafe { write_log_grow_push_wide(&mut buf, 0, payload.as_ptr(), 16) };
        assert!(ff_commit_from_log_watched(&mut ff, &buf, &mut { watched }));
        assert_eq!(&ff[0..16], &payload);
        // Re-commit of the same bytes: no report.
        assert!(!ff_commit_from_log_watched(&mut ff, &buf, &mut { watched }));

        // Out-of-bounds entries are skipped like `ff_commit_from_log`.
        let mut buf = WriteLogBuffer::with_capacity(4, 2);
        unsafe { write_log_grow_push_narrow(&mut buf, 63, 0xff, 8) };
        assert!(!ff_commit_from_log_watched(&mut ff, &buf, &mut {
            |_, _| true
        }));
    }

    #[test]
    fn push_helper_noop_when_inactive() {
        // Defensive: verify no segfault when TLS unset (e.g., very early
        // init path).
        unsafe {
            clear_event_write_log();
            event_write_log_push_static(0, 0, 0);
        }
    }
}
