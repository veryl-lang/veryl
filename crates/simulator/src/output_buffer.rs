//! Thread-local output buffer for `$display` / `$write` statements.
//! Prevents interleaved output during parallel test execution.

use std::cell::{Cell, RefCell};

thread_local! {
    static BUFFER: RefCell<Option<String>> = const { RefCell::new(None) };
    static MUTED: Cell<bool> = const { Cell::new(false) };
}

/// While alive, drops `$display` / `$write` output and assertion records: a
/// reference rerun of statements whose effects already happened once.
pub struct Mute(bool);

impl Mute {
    pub fn begin() -> Self {
        Mute(MUTED.replace(true))
    }
}

impl Drop for Mute {
    fn drop(&mut self) {
        MUTED.set(self.0);
    }
}

pub fn muted() -> bool {
    MUTED.get()
}

pub fn enable() {
    BUFFER.with(|b| {
        *b.borrow_mut() = Some(String::new());
    });
}

pub fn take() -> String {
    BUFFER.with(|b| b.borrow_mut().take().unwrap_or_default())
}

/// Byte length of the buffered output — a rollback point for a speculative
/// run (the AOT-C validate dual-run) whose `$display` output must not land
/// twice.  Unbuffered output cannot roll back; 0 keeps `truncate_to` a no-op.
pub fn mark() -> usize {
    BUFFER.with(|b| b.borrow().as_ref().map_or(0, |buf| buf.len()))
}

/// Drop everything buffered after `mark`.  No-op when unbuffered.
pub fn truncate_to(mark: usize) {
    BUFFER.with(|b| {
        if let Some(buf) = b.borrow_mut().as_mut()
            && mark <= buf.len()
        {
            buf.truncate(mark);
        }
    });
}

pub fn print(s: &str) {
    if muted() {
        return;
    }
    BUFFER.with(|b| {
        let mut borrow = b.borrow_mut();
        if let Some(buf) = borrow.as_mut() {
            buf.push_str(s);
        } else {
            print!("{s}");
        }
    });
}

pub fn println(s: &str) {
    if muted() {
        return;
    }
    BUFFER.with(|b| {
        let mut borrow = b.borrow_mut();
        if let Some(buf) = borrow.as_mut() {
            buf.push_str(s);
            buf.push('\n');
        } else {
            println!("{s}");
        }
    });
}
