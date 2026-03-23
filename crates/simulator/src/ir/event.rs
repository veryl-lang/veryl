use crate::ir::VarId;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Event {
    Clock(VarId),
    Reset(VarId),
    Initial,
    Final,
}

impl Event {
    pub fn var_id(&self) -> Option<VarId> {
        match self {
            Event::Clock(id) | Event::Reset(id) if *id != VarId::SYNTHETIC => Some(*id),
            _ => None,
        }
    }
}
