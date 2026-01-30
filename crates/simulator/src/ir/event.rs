use crate::ir::VarId;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Event {
    Clock(VarId),
    Reset(VarId),
    Initial,
    Final,
}
