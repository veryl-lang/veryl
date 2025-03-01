pub mod analyzer;
pub mod analyzer_error;
pub mod attribute;
pub mod attribute_table;
pub mod definition_table;
pub mod evaluator;
pub mod handlers;
pub mod instance_history;
pub mod msb_table;
pub mod multi_sources;
pub mod namespace;
pub mod namespace_table;
pub mod range_table;
pub mod reference_table;
pub mod symbol;
pub mod symbol_path;
pub mod symbol_table;
pub mod type_dag;
pub mod r#unsafe;
pub mod unsafe_table;
pub mod var_ref;
pub use analyzer::Analyzer;
pub use analyzer_error::AnalyzerError;
#[cfg(test)]
mod tests;

pub use smallvec::smallvec as svec;
pub type SVec<T> = smallvec::SmallVec<[T; 8]>;
