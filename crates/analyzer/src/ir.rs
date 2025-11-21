pub mod bigint;
mod declaration;
mod expression;
#[allow(clippy::module_inception)]
mod ir;
mod module;
mod statement;
mod typed_value;
mod value;
mod variable;
pub use declaration::{
    CombDeclaration, Declaration, DeclarationBlock, FfDeclaration, InstDeclaration,
};
pub use expression::{Expression, Factor, Op, Select};
pub use ir::{Component, Ir};
pub use module::Module;
pub use statement::{
    AssignStatement, ForStatement, IfResetStatement, IfStatement, Statement, StatementBlock,
};
pub use typed_value::{Type, TypeKind, TypedValue, UserDefined, ValueVariant};
pub use value::Value;
pub use variable::{VarId, VarIndex, VarKind, VarPath, VarPathIndex, Variable};

#[cfg(test)]
mod tests;
