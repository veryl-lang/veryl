//! Per-Ir FF write log buffer.
//!
//! NBA semantics: each FF write is recorded as a `WriteLogEntry` and applied
//! to `current` storage at cycle end by `ff_commit_from_log`.  This decouples
//! the commit cost from total FF count (favorable in sparse-write regimes).
//!
//! Entry layout: 16 bytes per entry.  Hot path stores 8-byte payload, 4-byte
//! offset, 2-byte mask_xz placeholder (used only in 4-state mode), and a
//! 2-byte width_class tag.

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct WriteLogEntry {
    /// FF current byte offset within ff_values where the payload should be applied.
    pub offset: u32,
    /// X/Z mask payload, used only when use_4state is true; zero otherwise.
    pub mask_xz: u16,
    /// Width class tag: 1, 2, 4, 8 for u8/u16/u32/u64; 0xFF for wide.
    /// Site id mapping back to SiteTable is recoverable via offset lookup.
    pub width_class: u16,
    /// Stored value.  For widths < 8 bytes the upper bits are zero or
    /// undefined; the consumer truncates by width_class.
    pub payload: u64,
}

/// Per-Ir write log buffer.  Sized at compile time from `SiteTable.len()`
/// upper bound (one entry per site per cycle).  Repeated writes to the
/// same site push extra entries; the consumer applies them in order so
/// last-write-wins.
///
/// Layout-stable for JIT access: `#[repr(C)]` guarantees the
/// field order and offsets so JIT-emitted code can read/write `entries_ptr`
/// at offset 0, `count` at offset 8, and `capacity` at offset 12 directly
/// (no helper call, no TLS).  `_entries_owner` keeps the heap allocation
/// alive; `entries_ptr` is cached at construction and stays valid for the
/// buffer's lifetime (`Box<[T]>` is never reallocated).
#[repr(C)]
#[derive(Debug)]
pub struct WriteLogBuffer {
    /// Raw pointer to entries[0].  Stable for buffer lifetime — JIT
    /// reads this at offset 0 to compute entry slot addresses.
    pub entries_ptr: *mut WriteLogEntry,
    /// Live count of entries written this cycle.  Reset to 0 by the
    /// consumer at end of commit.  JIT reads/writes at offset 8.
    pub count: u32,
    /// Capacity in entries.  Constant after construction.  JIT may
    /// read for debug bounds check at offset 12.
    pub capacity: u32,
    /// Owning storage — JIT never touches; keeps `entries_ptr` valid.
    _entries_owner: Box<[WriteLogEntry]>,
}

// SAFETY: WriteLogBuffer is owned by a single Ir which is bound to a
// single thread (Ir is Send but not Sync).
unsafe impl Send for WriteLogBuffer {}

/// Layout offsets baked into JIT codegen.  Any change here requires
/// matching updates in `cranelift.rs::emit_inline_write_log_push`.
/// Only meaningful on 64-bit native targets where the JIT is enabled;
/// on wasm32 the pointer-size-dependent offsets differ and the JIT is
/// disabled, so the constants and the static layout assertions are
/// gated out.
#[cfg(not(target_family = "wasm"))]
pub const WRITE_LOG_OFFSET_ENTRIES_PTR: i32 = 0;
#[cfg(not(target_family = "wasm"))]
pub const WRITE_LOG_OFFSET_COUNT: i32 = 8;
#[cfg(not(target_family = "wasm"))]
pub const WRITE_LOG_ENTRY_SIZE: i32 = 16;
#[cfg(not(target_family = "wasm"))]
pub const WRITE_LOG_ENTRY_OFFSET_OFFSET: i32 = 0;
#[cfg(not(target_family = "wasm"))]
pub const WRITE_LOG_ENTRY_OFFSET_MASK_XZ: i32 = 4;
#[cfg(not(target_family = "wasm"))]
pub const WRITE_LOG_ENTRY_OFFSET_WIDTH_CLASS: i32 = 6;
#[cfg(not(target_family = "wasm"))]
pub const WRITE_LOG_ENTRY_OFFSET_PAYLOAD: i32 = 8;

#[cfg(not(target_family = "wasm"))]
const _: () = {
    assert!(std::mem::offset_of!(WriteLogBuffer, entries_ptr) == 0);
    assert!(std::mem::offset_of!(WriteLogBuffer, count) == 8);
    assert!(std::mem::offset_of!(WriteLogBuffer, capacity) == 12);
    assert!(std::mem::offset_of!(WriteLogEntry, offset) == 0);
    assert!(std::mem::offset_of!(WriteLogEntry, mask_xz) == 4);
    assert!(std::mem::offset_of!(WriteLogEntry, width_class) == 6);
    assert!(std::mem::offset_of!(WriteLogEntry, payload) == 8);
    assert!(std::mem::size_of::<WriteLogEntry>() == 16);
};

impl Default for WriteLogBuffer {
    fn default() -> Self {
        Self::with_capacity(0)
    }
}

impl WriteLogBuffer {
    /// Allocate a buffer with `capacity` slots, all zero-initialized.
    pub fn with_capacity(capacity: usize) -> Self {
        let mut entries = vec![WriteLogEntry::default(); capacity].into_boxed_slice();
        let entries_ptr = entries.as_mut_ptr();
        Self {
            entries_ptr,
            count: 0,
            capacity: capacity as u32,
            _entries_owner: entries,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity as usize
    }

    pub fn count(&self) -> u32 {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn reset(&mut self) {
        self.count = 0;
    }

    /// Safe slice view of the live entries (used by `ff_commit_from_log`
    /// and tests).
    pub fn entries_slice(&self) -> &[WriteLogEntry] {
        // SAFETY: entries_ptr points to a Box<[WriteLogEntry]> of length
        // capacity; count <= capacity by construction.
        unsafe { std::slice::from_raw_parts(self.entries_ptr, self.capacity as usize) }
    }
}

/// Apply each `WriteLogEntry`'s payload to the FF current slot.  The buffer
/// is iterated in insertion order so multiple writes to the same offset
/// apply last-write-wins, matching JIT/interpret semantics.
pub fn ff_commit_from_log(ff_values: &mut [u8], buffer: &WriteLogBuffer) {
    let limit = buffer.count as usize;
    let len = ff_values.len();
    let dst = ff_values.as_mut_ptr();
    for entry in buffer.entries_slice().iter().take(limit) {
        let nb = entry.width_class as usize;
        let offset = entry.offset as usize;
        if offset + nb > len {
            continue;
        }
        // Single word store per width class — keeps wide FF commit
        // throughput high (4 words for 256-bit ⇒ 4 stores, not 32).
        // SAFETY: bounds verified above; dst is the start of the slice.
        unsafe {
            let p = dst.add(offset);
            match nb {
                8 => (p as *mut u64).write_unaligned(entry.payload),
                4 => (p as *mut u32).write_unaligned(entry.payload as u32),
                2 => (p as *mut u16).write_unaligned(entry.payload as u16),
                1 => *p = entry.payload as u8,
                _ => continue,
            }
        }
    }
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

/// Push a static FF write entry into the active log buffer.  Called from
/// JIT code (`extern "C"`) and from the interpret path.  Width class is
/// one of 1/2/4/8 (== native bytes) or 0xFF for wide-FF marker.
///
/// Safety: caller is the JIT-emitted code which only invokes this while
/// the TLS is installed by `set_event_write_log`.  Overflow asserts in
/// debug builds; releases truncate-wrap (acceptable since `write_log_capacity`
/// over-provisions, plan calls for grow-on-overflow as a follow-up).
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
        let idx = buf.count as usize;
        let cap = buf.capacity as usize;
        debug_assert!(idx < cap, "write_log overflow: idx={} cap={}", idx, cap,);
        if idx < cap {
            // SAFETY: entries_ptr points to capacity-sized allocation,
            // idx < cap, so the offset is in bounds.
            unsafe {
                *buf.entries_ptr.add(idx) = WriteLogEntry {
                    offset,
                    mask_xz: 0,
                    width_class,
                    payload,
                };
            }
            buf.count = (idx as u32).saturating_add(1);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_layout_is_16_bytes() {
        assert_eq!(core::mem::size_of::<WriteLogEntry>(), 16);
    }

    #[test]
    fn empty_buffer_has_zero_count() {
        let b = WriteLogBuffer::with_capacity(0);
        assert_eq!(b.capacity(), 0);
        assert_eq!(b.count(), 0);
        assert!(b.is_empty());
    }

    #[test]
    fn capacity_allocated() {
        let b = WriteLogBuffer::with_capacity(16);
        assert_eq!(b.capacity(), 16);
        assert_eq!(b.count(), 0);
    }

    #[test]
    fn reset_clears_count() {
        let mut b = WriteLogBuffer::with_capacity(4);
        b.count = 3;
        b.reset();
        assert_eq!(b.count, 0);
    }

    #[test]
    fn push_helper_writes_into_active_buffer() {
        let mut buf = WriteLogBuffer::with_capacity(4);
        unsafe {
            set_event_write_log(&mut buf);
            event_write_log_push_static(0x1000, 0xdead_beef, 8);
            event_write_log_push_static(0x1008, 0xfeed_face, 4);
            clear_event_write_log();
        }
        assert_eq!(buf.count, 2);
        let entries = buf.entries_slice();
        assert_eq!(entries[0].offset, 0x1000);
        assert_eq!(entries[0].payload, 0xdead_beef);
        assert_eq!(entries[0].width_class, 8);
        assert_eq!(entries[1].offset, 0x1008);
        assert_eq!(entries[1].payload, 0xfeed_face);
        assert_eq!(entries[1].width_class, 4);
    }

    #[test]
    fn push_helper_noop_when_inactive() {
        // Defensive: verify no segfault when TLS unset (e.g., very early
        // init path).  buf.count must remain 0 elsewhere; this just calls
        // the helper without an installed buffer.
        unsafe {
            clear_event_write_log();
            event_write_log_push_static(0, 0, 0);
        }
    }
}
