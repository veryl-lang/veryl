//! Thread-local output buffer for `$display` / `$write` statements.
//! Prevents interleaved output during parallel test execution.

use std::cell::RefCell;

thread_local! {
    static BUFFER: RefCell<Option<String>> = const { RefCell::new(None) };
}

pub fn enable() {
    BUFFER.with(|b| {
        *b.borrow_mut() = Some(String::new());
    });
}

pub fn take() -> String {
    BUFFER.with(|b| b.borrow_mut().take().unwrap_or_default())
}

pub fn print(s: &str) {
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
