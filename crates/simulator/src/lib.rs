pub mod assert_buffer;
pub mod backend;
pub mod file_table;
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

// 4th arg `ff_delta`: byte delta from the base the chunk was compiled at to this
// instance's ff base, added to baked FF write-log offsets so a relocated
// (cache-reused) chunk records absolute `ff_values` offsets. 0 when not reused.
pub type FuncPtr = unsafe extern "system" fn(*const u8, *const u8, *mut u8, isize);

#[cfg(test)]
mod tests;

type HashMap<K, V> = fxhash::FxHashMap<K, V>;
type HashSet<V> = fxhash::FxHashSet<V>;
