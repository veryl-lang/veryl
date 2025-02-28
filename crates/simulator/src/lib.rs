pub struct Simulator;

impl Simulator {
    pub fn new(_top: &str) -> Self {
        Self
    }

    pub fn set(&mut self, _port: &str, _value: usize) {}

    pub fn get(&mut self, _port: &str) -> usize {
        0
    }

    pub fn step(&mut self) {}
}

#[cfg(test)]
mod tests;
