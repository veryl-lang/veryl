use crate::HashMap;
use std::cell::RefCell;
use std::time::{Duration, Instant};

pub struct Stopwatch {
    start: Option<Instant>,
    total: Duration,
}

thread_local!(static STOPWATCH_TABLE: RefCell<HashMap<&'static str, Stopwatch>> = RefCell::new(HashMap::default()));

pub fn start(key: &'static str) {
    STOPWATCH_TABLE.with(|f| {
        f.borrow_mut()
            .entry(key)
            .and_modify(|x| {
                x.start = Some(Instant::now());
            })
            .or_insert(Stopwatch {
                start: Some(Instant::now()),
                total: Duration::default(),
            });
    });
}

pub fn stop(key: &'static str) {
    STOPWATCH_TABLE.with(|f| {
        f.borrow_mut()
            .entry(key)
            .and_modify(|x| {
                if let Some(start) = x.start {
                    x.total += start.elapsed();
                }
                x.start = None;
            })
            .or_insert(Stopwatch {
                start: None,
                total: Duration::default(),
            });
    });
}

pub fn dump() {
    STOPWATCH_TABLE.with(|f| {
        for (k, v) in f.borrow().iter() {
            println!("{k}: {} us", v.total.as_micros());
        }
    });
}
