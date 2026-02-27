mod assign_table;
mod comptime;
mod declaration;
mod expression;
mod ff_table;
mod function;
mod interface;
#[allow(clippy::module_inception)]
mod ir;
mod module;
mod op;
mod shape;
mod signature;
mod statement;
mod system_function;
mod utils;
mod variable;
pub use comptime::{
    Comptime, ExpressionContext, PartSelectPath, Type, TypeKind, TypeKindEnum, TypeKindMember,
    TypeKindStruct, TypeKindUnion, ValueVariant,
};
pub use declaration::{
    CombDeclaration, Declaration, DeclarationBlock, FfClock, FfDeclaration, FfReset,
    FinalDeclaration, InitialDeclaration, InstDeclaration, InstInput, InstOutput,
};
pub use expression::{ArrayLiteralItem, Expression, Factor};
pub use ff_table::FfTable;
pub use function::{Arguments, FuncArg, FuncPath, FuncProto, Function, FunctionBody, FunctionCall};
pub use interface::Interface;
pub use ir::{Component, Ir, IrError, IrResult, SystemVerilog};
pub use module::Module;
pub use op::Op;
pub use shape::{Shape, ShapeRef};
pub use signature::Signature;
pub use statement::{
    AssignDestination, AssignStatement, IfResetStatement, IfStatement, Statement, StatementBlock,
};
pub use system_function::SystemFunctionCall;
pub use variable::{
    VarId, VarIndex, VarKind, VarPath, VarPathSelect, VarSelect, VarSelectOp, Variable,
    VariableInfo,
};

#[cfg(test)]
mod tests;
