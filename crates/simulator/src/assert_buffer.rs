//! Thread-local buffer for `$assert` / `$assert_continue` failures.
//!
//! Using a thread-local lets the combinational evaluation path record
//! failures without panicking.

use std::cell::RefCell;

#[derive(Default)]
struct State {
    fatal: Option<String>,
    continues: Vec<String>,
}

thread_local! {
    static STATE: RefCell<State> = const {
        RefCell::new(State {
            fatal: None,
            continues: Vec::new(),
        })
    };
}

pub fn reset() {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.fatal = None;
        s.continues.clear();
    });
}

pub fn record_fatal(msg: String) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if s.fatal.is_none() {
            s.fatal = Some(msg);
        }
    });
}

pub fn record_continue(msg: String) {
    STATE.with(|s| s.borrow_mut().continues.push(msg));
}

pub fn has_fatal() -> bool {
    STATE.with(|s| s.borrow().fatal.is_some())
}

pub fn take_failure() -> Option<String> {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if let Some(msg) = s.fatal.take() {
            s.continues.clear();
            return Some(msg);
        }
        if s.continues.is_empty() {
            None
        } else {
            let msgs = std::mem::take(&mut s.continues);
            Some(msgs.join("\n"))
        }
    })
}
