pub mod assert_buffer;
pub mod backend;
pub mod component;
pub mod file_table;
pub mod ir;
pub mod output_buffer;
pub mod random_table;
pub mod residency;
pub mod simulator;
pub mod simulator_error;
pub mod testbench;
pub mod wave_dumper;
pub mod wavedrom;
pub mod wide_ops;

pub use ir::Config;
pub use simulator::Simulator;
pub use simulator_error::SimulatorError;

/// Stack size for threads that walk user-design IR (conv over nested
/// statements, the AOT-C expression emitter): the recursion depth is bounded
/// by design nesting, but a deep decoder chain exceeds the 2 MiB
/// spawned-thread default under debug frame sizes.  Reserved virtual memory
/// only — untouched pages are never committed.
pub const IR_WALK_STACK_BYTES: usize = 64 * 1024 * 1024;

// 4th arg `ff_delta`: byte delta from the base the chunk was compiled at to this
// instance's ff base, added to baked FF write-log offsets so a relocated
// (cache-reused) chunk records absolute `ff_values` offsets. 0 when not reused.
pub type FuncPtr = unsafe extern "system" fn(*const u8, *const u8, *mut u8, isize);

#[cfg(test)]
mod tests;

type HashMap<K, V> = fxhash::FxHashMap<K, V>;
type HashSet<V> = fxhash::FxHashSet<V>;
