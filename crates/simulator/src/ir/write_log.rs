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
    /// Narrow capacity (constant after construction).
    pub narrow_capacity: u32,
    /// Wide path: pointer to the [`WriteLogWideEntry`] array (64 B per entry).
    pub wide_entries_ptr: *mut WriteLogWideEntry,
    /// Live wide-entry count for this cycle.
    pub wide_count: u32,
    /// Wide capacity (constant after construction).
    pub wide_capacity: u32,
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
pub const WRITE_LOG_WIDE_OFFSET_ENTRIES_PTR: i32 =
    std::mem::offset_of!(WriteLogBuffer, wide_entries_ptr) as i32;
#[allow(dead_code)]
pub const WRITE_LOG_WIDE_OFFSET_COUNT: i32 =
    std::mem::offset_of!(WriteLogBuffer, wide_count) as i32;

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
            _narrow_owner: narrow,
            _wide_owner: wide,
        }
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

/// Apply each log entry's payload to the FF current slot.  Narrow entries
/// are applied first, then wide entries.  Within each pool, entries are
/// processed in insertion order so multiple writes to the same offset
/// apply last-write-wins, matching JIT/interpret semantics.
pub fn ff_commit_from_log(ff_values: &mut [u8], buffer: &WriteLogBuffer) {
    let len = ff_values.len();
    let dst = ff_values.as_mut_ptr();

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
                8 => (p as *mut u64).write_unaligned(entry.payload),
                4 => (p as *mut u32).write_unaligned(entry.payload as u32),
                2 => (p as *mut u16).write_unaligned(entry.payload as u16),
                1 => *p = entry.payload as u8,
                _ => continue,
            }
        }
    }

    // Wide path: memcpy of the embedded payload.
    let wide_limit = buffer.wide_count as usize;
    for entry in buffer.wide_entries_slice().iter().take(wide_limit) {
        let nb = entry.native_bytes as usize;
        let offset = entry.offset as usize;
        if nb == 0 || nb > WRITE_LOG_WIDE_ENTRY_PAYLOAD_BYTES || offset + nb > len {
            continue;
        }
        ff_values[offset..offset + nb].copy_from_slice(&entry.payload[..nb]);
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

/// Push a narrow FF write entry into the active log buffer.  Called from
/// JIT code (`extern "C"`) and from the interpret path.  Width class is
/// one of 1/2/4/8 (== native bytes).
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
        let idx = buf.narrow_count as usize;
        let cap = buf.narrow_capacity as usize;
        debug_assert!(
            idx < cap,
            "write_log narrow overflow: idx={} cap={}",
            idx,
            cap
        );
        if idx < cap {
            // SAFETY: narrow_entries_ptr points to a capacity-sized allocation;
            // idx < cap so the offset is in bounds.
            unsafe {
                *buf.narrow_entries_ptr.add(idx) = WriteLogEntry {
                    offset,
                    mask_xz: 0,
                    width_class,
                    payload,
                };
            }
            buf.narrow_count = (idx as u32).saturating_add(1);
        }
    });
}

/// Push a wide FF write entry (used by the interpret path).  `payload`
/// must point to `native_bytes` (≤ 56) bytes of FF data.
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
        let idx = buf.wide_count as usize;
        let cap = buf.wide_capacity as usize;
        debug_assert!(
            idx < cap,
            "write_log wide overflow: idx={} cap={}",
            idx,
            cap
        );
        if idx < cap {
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
            unsafe {
                *buf.wide_entries_ptr.add(idx) = entry;
            }
            buf.wide_count = (idx as u32).saturating_add(1);
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
