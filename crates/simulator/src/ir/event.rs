use crate::ir::VarId;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Event {
    Clock(VarId),
    Reset(VarId),
    Initial,
    /// The `n`th `initial` block, `n >= 1` (block 0 is `Initial`), kept
    /// apart so the testbench can run it as its own process.
    InitialBlock(u32),
    Final,
}

impl Event {
    pub fn var_id(&self) -> Option<VarId> {
        match self {
            Event::Clock(id) | Event::Reset(id) if *id != VarId::SYNTHETIC => Some(*id),
            _ => None,
        }
    }

    pub fn initial_index(&self) -> Option<u32> {
        match self {
            Event::Initial => Some(0),
            Event::InitialBlock(n) => Some(*n),
            _ => None,
        }
    }

    pub fn is_initial(&self) -> bool {
        self.initial_index().is_some()
    }

    pub fn next_initial(existing: u32) -> Event {
        if existing == 0 {
            Event::Initial
        } else {
            Event::InitialBlock(existing)
        }
    }
}
