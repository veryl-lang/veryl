pub mod cranelift;
pub mod ir;
pub mod simulator;
pub mod wide_ops;

pub use ir::Config;
pub use simulator::Simulator;

#[cfg(test)]
mod tests;

type HashMap<K, V> = fxhash::FxHashMap<K, V>;
type HashSet<V> = fxhash::FxHashSet<V>;
