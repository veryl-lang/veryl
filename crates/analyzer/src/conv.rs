pub mod checker;
mod context;
pub mod declaration;
pub mod expression;
pub mod instance;
pub mod ir;
pub mod statement;
pub mod utils;
pub mod var;
pub use context::{Context, EvalContext};

use crate::ir::IrResult;
use crate::symbol::Affiliation;

pub trait Conv<T>: Sized {
    fn conv(context: &mut Context, value: T) -> IrResult<Self>;
}
