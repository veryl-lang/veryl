pub mod checker;
mod context;
pub mod conv_profiler;
pub mod declaration;
pub mod expression;
pub mod instance;
pub mod ir;
pub mod statement;
pub mod utils;
pub mod var;
pub use context::Context;

use crate::ir::IrResult;
use crate::symbol::Affiliation;
use veryl_parser::resource_table::{self, StrId};

pub trait Conv<T>: Sized {
    fn conv(context: &mut Context, value: T) -> IrResult<Self>;
}

/// The `label[index]` segment for one generate-block iteration (e.g. `g_leaf[0]`).
/// Shared by block elaboration and hierarchical-reference folding so both name
/// the iteration identically.
pub fn generate_block_label(label: StrId, index: usize) -> StrId {
    resource_table::insert_str(&format!("{label}[{index}]"))
}
