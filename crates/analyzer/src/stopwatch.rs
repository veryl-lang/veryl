use crate::HashMap;
use std::cell::RefCell;
use std::time::{Duration, Instant};

#[derive(Clone, Default)]
pub struct Stopwatch {
    start: Option<Instant>,
    count: usize,
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
                count: 0,
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
                x.count += 1;
                x.start = None;
            })
            .or_insert(Stopwatch {
                start: None,
                count: 0,
                total: Duration::default(),
            });
    });
}

pub fn dump() {
    for _ in 0..100 {
        start("calc_base");
        stop("calc_base");
    }
    let base = STOPWATCH_TABLE
        .with(|f| f.borrow().get("calc_base").unwrap().total.as_nanos() as f64 / 100.0);
    STOPWATCH_TABLE.with(|f| f.borrow_mut().remove("calc_base"));

    STOPWATCH_TABLE.with(|f| {
        let width = f.borrow().iter().map(|x| x.0.len()).max().unwrap_or(0);
        for (k, v) in f.borrow().iter() {
            let iter = (v.total.as_nanos() as f64 / v.count as f64) - base;
            let iter = if iter < 0.0 { 0.0 } else { iter };
            println!(
                "{k:width$}: {} us, {} iter, {:.3} ns / iter",
                v.total.as_micros(),
                v.count,
                iter,
            );
        }
    });
}
