//! Thread-local file handle table backing the `$tb::file` testbench handles.
//!
//! Like `output_buffer`/`assert_buffer`, a thread-local keeps the open-file
//! state reachable from the testbench driver (which has no `Simulator` handle)
//! and isolates parallel tests. Handles are keyed by the declaring variable's
//! name (`StrId`): `f.open(...)` is a statement with no return value, so there
//! is no descriptor for the user to hold.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use veryl_parser::resource_table::{self, StrId};

#[derive(Default)]
struct FileTable {
    files: HashMap<StrId, BufWriter<File>>,
    /// Handles already warned about a write-before-open, so the diagnostic is
    /// emitted once per handle instead of on every dropped write in a loop.
    warned: HashSet<StrId>,
}

thread_local! {
    static TABLE: RefCell<FileTable> = RefCell::new(FileTable::default());
}

/// Clear all handles. Call before a test.
pub fn reset() {
    TABLE.with(|t| {
        let mut t = t.borrow_mut();
        t.files.clear();
        t.warned.clear();
    });
}

/// Open `path` for handle `key`, truncating (`append = false`) or appending
/// (`append = true`). Re-opening a live handle closes the previous file first.
/// A failed open leaves the handle unbound, so later writes are dropped.
pub fn open_handle(key: StrId, path: &str, append: bool) {
    close_handle(key);
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(!append)
        .append(append)
        .open(path);
    if let Ok(file) = file {
        TABLE.with(|t| {
            t.borrow_mut().files.insert(key, BufWriter::new(file));
        });
    }
}

/// Write `s` to handle `key`. An unbound handle (never opened, or a failed
/// open) drops the write, matching SystemVerilog's handling of a write to an
/// invalid descriptor, and warns once per handle since the dropped data is
/// usually a mistake.
pub fn write_handle(key: StrId, s: &str) {
    let warn = TABLE.with(|t| {
        let mut t = t.borrow_mut();
        if let Some(w) = t.files.get_mut(&key) {
            let _ = w.write_all(s.as_bytes());
            false
        } else {
            t.warned.insert(key)
        }
    });
    if warn {
        let name = resource_table::get_str_value(key).unwrap_or_default();
        log::warn!("$tb::file handle '{name}' written before open(); write ignored");
    }
}

/// Flush and close handle `key`. Unknown handles are ignored.
pub fn close_handle(key: StrId) {
    TABLE.with(|t| {
        if let Some(mut w) = t.borrow_mut().files.remove(&key) {
            let _ = w.flush();
        }
    });
}

/// Flush handle `key`. Unknown handles are ignored.
pub fn flush_handle(key: StrId) {
    TABLE.with(|t| {
        if let Some(w) = t.borrow_mut().files.get_mut(&key) {
            let _ = w.flush();
        }
    });
}

/// Flush and close every open handle. Call at simulation end so buffered output
/// is not lost when a testbench forgets to close a handle.
pub fn finalize() {
    TABLE.with(|t| {
        let mut t = t.borrow_mut();
        for (_, mut w) in t.files.drain() {
            let _ = w.flush();
        }
        t.warned.clear();
    });
}
