//! Thread-local file handle table for the `$f*` file-write system tasks.
//!
//! Like `output_buffer`/`assert_buffer`, a thread-local keeps the open-file
//! state reachable from `eval_step` (which has no `Simulator` handle) and
//! isolates parallel tests. Descriptors 1/2 route to `output_buffer`.

use crate::output_buffer;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};

struct FileTable {
    next_fd: i32,
    files: HashMap<i32, BufWriter<File>>,
}

impl FileTable {
    fn new() -> Self {
        // 0 = error/invalid, 1 = STDOUT, 2 = STDERR; user files start at 3.
        Self {
            next_fd: 3,
            files: HashMap::new(),
        }
    }
}

thread_local! {
    static TABLE: RefCell<FileTable> = RefCell::new(FileTable::new());
}

/// Clear all handles and reset the descriptor counter. Call before a test.
pub fn reset() {
    TABLE.with(|t| {
        let mut t = t.borrow_mut();
        t.files.clear();
        t.next_fd = 3;
    });
}

/// Open `path` in fopen-style `mode`. Returns a descriptor (>= 3), or 0 on
/// failure or a non-write mode. `a*` appends, `w*` truncates; read modes are
/// rejected with 0 so this write-only facility never truncates them. "b" is
/// ignored.
pub fn open(path: &str, mode: &str) -> i32 {
    let append = mode.starts_with('a');
    let truncate = mode.starts_with('w');
    if !append && !truncate {
        return 0;
    }
    let mut opts = OpenOptions::new();
    opts.write(true).create(true);
    if append {
        opts.append(true);
    } else {
        opts.truncate(true);
    }
    match opts.open(path) {
        Ok(file) => TABLE.with(|t| {
            let mut t = t.borrow_mut();
            let fd = t.next_fd;
            t.next_fd += 1;
            t.files.insert(fd, BufWriter::new(file));
            fd
        }),
        Err(_) => 0,
    }
}

/// Write `s` to `fd`. Descriptors 1/2 (STDOUT/STDERR) route to the shared
/// output buffer — there is no separate stderr channel. An unknown `fd`
/// (including 0 from a failed `$fopen`) is dropped, like SystemVerilog.
pub fn write(fd: i32, s: &str) {
    if fd == 1 || fd == 2 {
        output_buffer::print(s);
        return;
    }
    TABLE.with(|t| {
        let mut t = t.borrow_mut();
        if let Some(w) = t.files.get_mut(&fd) {
            let _ = w.write_all(s.as_bytes());
        }
    });
}

/// Flush and close `fd`. Unknown / reserved descriptors are ignored.
pub fn close(fd: i32) {
    TABLE.with(|t| {
        let mut t = t.borrow_mut();
        if let Some(mut w) = t.files.remove(&fd) {
            let _ = w.flush();
        }
    });
}

/// Flush `fd` (or every open file when `fd` is `None`).
pub fn flush(fd: Option<i32>) {
    TABLE.with(|t| {
        let mut t = t.borrow_mut();
        match fd {
            Some(fd) => {
                if let Some(w) = t.files.get_mut(&fd) {
                    let _ = w.flush();
                }
            }
            None => {
                for w in t.files.values_mut() {
                    let _ = w.flush();
                }
            }
        }
    });
}

/// Flush and close every open file. Call at simulation end so buffered
/// output is not lost when a testbench forgets to `$fclose`.
pub fn finalize() {
    TABLE.with(|t| {
        let mut t = t.borrow_mut();
        for (_, mut w) in t.files.drain() {
            let _ = w.flush();
        }
        t.next_fd = 3;
    });
}
