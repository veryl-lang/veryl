mod assign_table;
pub mod bigint;
mod declaration;
mod expression;
mod function;
mod interface;
#[allow(clippy::module_inception)]
mod ir;
mod module;
mod statement;
mod system_function;
mod typed_value;
mod utils;
mod value;
mod variable;
pub use declaration::{
    CombDeclaration, Declaration, DeclarationBlock, FfDeclaration, InstDeclaration,
};
pub use expression::{ArrayLiteralItem, Expression, Factor, Op};
pub use function::{Arguments, Function, FunctionBody, FunctionCall};
pub use interface::Interface;
pub use ir::{Component, Ir};
pub use module::Module;
pub use statement::{
    AssignDestination, AssignStatement, IfResetStatement, IfStatement, Statement, StatementBlock,
    SystemFunctionCall,
};
pub use system_function::SystemFunctionKind;
pub use typed_value::{Type, TypeKind, TypedValue, ValueVariant};
pub use value::Value;
pub use variable::{
    VarId, VarIndex, VarKind, VarPath, VarPathSelect, VarSelect, VarSelectOp, Variable,
};

#[cfg(test)]
mod tests;
