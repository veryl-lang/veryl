use std::time::Instant;

pub struct StopWatch {
    now: Instant,
}

impl Default for StopWatch {
    fn default() -> Self {
        Self {
            now: Instant::now(),
        }
    }
}

impl StopWatch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn lap(&mut self) -> u128 {
        let ret = self.now.elapsed().as_millis();
        self.now = Instant::now();
        ret
    }
}
