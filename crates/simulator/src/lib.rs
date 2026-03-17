pub mod cranelift;
pub mod ir;
pub mod simulator;
pub mod simulator_error;
pub mod testbench;
pub mod wide_ops;

pub use ir::Config;
pub use simulator::Simulator;
pub use simulator_error::SimulatorError;

#[cfg(test)]
mod tests;

type HashMap<K, V> = fxhash::FxHashMap<K, V>;
type HashSet<V> = fxhash::FxHashSet<V>;
