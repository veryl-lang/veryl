pub mod assert_buffer;
pub mod backend;
pub mod ir;
pub mod output_buffer;
pub mod simulator;
pub mod simulator_error;
pub mod testbench;
pub mod wave_dumper;
pub mod wavedrom;
pub mod wide_ops;

pub use ir::Config;
pub use simulator::Simulator;
pub use simulator_error::SimulatorError;

pub type FuncPtr = unsafe extern "system" fn(*const u8, *const u8, *mut u8);

#[cfg(test)]
mod tests;

type HashMap<K, V> = fxhash::FxHashMap<K, V>;
type HashSet<V> = fxhash::FxHashSet<V>;
