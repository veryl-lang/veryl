use std::time::Instant;

pub struct StopWatch {
    now: Instant,
}

impl StopWatch {
    pub fn new() -> Self {
        Self {
            now: Instant::now(),
        }
    }

    pub fn lap(&mut self) -> u128 {
        let ret = self.now.elapsed().as_millis();
        self.now = Instant::now();
        ret
    }
}
