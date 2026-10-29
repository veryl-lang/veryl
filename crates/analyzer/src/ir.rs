mod assign_table;
pub mod comb_to_ff_hoist;
mod comptime;
mod declaration;
mod expression;
mod ff_table;
mod function;
mod hierarchy;
pub mod peel;
pub(crate) use function::FunctionValueCache;
#[cfg(test)]
pub(crate) use function::VALUE_EVALUATIONS;
mod interface;
#[allow(clippy::module_inception)]
mod ir;
mod module;
mod op;
mod shape;
mod signature;
pub mod span;
mod statement;
mod system_function;
mod utils;
mod variable;
mod width_expr;
mod write_count;
mod write_summary;
pub(crate) use assign_table::AssignTable;
pub use assign_table::{AssignTableEntry, ReferencedEntry};
pub use comptime::{
    Comptime, ExpressionContext, InstanceKind, MemberSelectDomain, PartSelectPath, Type, TypeKind,
    TypeKindEnum, TypeKindMember, TypeKindStruct, TypeKindUnion, ValueVariant,
};
pub use declaration::{
    CombDeclaration, Declaration, DeclarationBlock, ExternalConnect, ExternalDeclaration,
    ExternalParamValue, FfClock, FfDeclaration, FfReset, FinalDeclaration, InitialDeclaration,
    InstDeclaration, InstInput, InstInterfaceBinding, InstOutput, qualified_prefix_len,
};
pub use expression::{
    ArrayLiteralItem, Expression, Factor, HierVarRef, ResetPolarity, ResetValueKind,
};
pub use ff_table::FfTable;
pub use function::{Arguments, FuncArg, FuncPath, FuncProto, Function, FunctionBody, FunctionCall};
pub use interface::Interface;
pub use ir::{Component, Ir, IrError, IrResult, SystemVerilog};
pub use module::Module;
pub use op::Op;
pub use shape::{Shape, ShapeRef};
pub use signature::{Signature, stable_type};
pub use statement::{
    AssignDestination, AssignStatement, CaseArm, CasePattern, CaseStatement, ControlFlow, ForBound,
    ForRange, ForStatement, HierAssignDestination, IfResetStatement, IfStatement, QueueOp,
    Statement, StatementBlock, TbMethod, TbMethodCall,
};
pub use system_function::{
    AssertKind, Input as SystemFunctionInput, Output as SystemFunctionOutput, SystemFunctionCall,
    SystemFunctionKind,
};
pub use utils::convert_cast;
pub use variable::{
    VarId, VarIndex, VarKind, VarPath, VarPathSelect, VarSelect, VarSelectOp, Variable,
    VariableInfo,
};
pub use width_expr::{WidthExpr, WidthOp};
pub use write_summary::{ProcessWrites, RegionWrite, WriteSummary};

#[cfg(test)]
mod tests;
