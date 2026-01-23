use crate::ir::VarId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Clock(VarId),
    Reset(VarId),
    Initial,
    Final,
}
