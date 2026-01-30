pub mod conv;
pub mod cranelift;
pub mod ir;
pub mod simulator;

pub use conv::Config;
pub use simulator::Simulator;

#[cfg(test)]
mod tests;

type HashMap<K, V> = fxhash::FxHashMap<K, V>;
