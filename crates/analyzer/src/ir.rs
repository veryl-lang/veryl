mod assign_table;
mod comptime;
mod declaration;
mod expression;
mod function;
mod interface;
#[allow(clippy::module_inception)]
mod ir;
mod module;
mod signature;
mod statement;
mod system_function;
mod utils;
mod variable;
pub use comptime::{
    Comptime, Type, TypeKind, TypeKindEnum, TypeKindMember, TypeKindStruct, ValueVariant,
};
pub use declaration::{
    CombDeclaration, Declaration, DeclarationBlock, FfClock, FfDeclaration, FfReset,
    FinalDeclaration, InitialDeclaration, InstDeclaration,
};
pub use expression::{ArrayLiteralItem, Expression, Factor, Op};
pub use function::{Arguments, FuncArg, FuncPath, FuncProto, Function, FunctionBody, FunctionCall};
pub use interface::Interface;
pub use ir::{Component, Ir, IrError, IrResult, SystemVerilog};
pub use module::Module;
pub use signature::Signature;
pub use statement::{
    AssignDestination, AssignStatement, IfResetStatement, IfStatement, Statement, StatementBlock,
};
pub use system_function::SystemFunctionCall;
pub use variable::{
    VarId, VarIndex, VarKind, VarPath, VarPathSelect, VarSelect, VarSelectOp, Variable,
};

#[cfg(test)]
mod tests;
