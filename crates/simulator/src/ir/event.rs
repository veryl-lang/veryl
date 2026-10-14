use crate::ir::VarId;

/// A clock or reset edge.  An unpacked `clock [N]` / `reset [N]` is one
/// `VarId` with N independent nets, so the element carries the second half
/// of the identity; without it every `always_ff (clk[k])` of one array
/// collapses onto a single event and fires on its siblings' edges.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Event {
    Clock(VarId, u32),
    Reset(VarId, u32),
    Initial,
    /// The `n`th `initial` block, `n >= 1` (block 0 is `Initial`), kept
    /// apart so the testbench can run it as its own process.
    InitialBlock(u32),
    Final,
}

impl Event {
    /// The edge of a net whose identity is already whole: a scalar, an
    /// array element handed a `VarId` of its own once its owner scope
    /// closed, or a top-level port array, which folds onto element 0
    /// because the caller drives all of it with one edge.  Everything past
    /// IR assembly sees only these.
    pub fn clock(id: VarId) -> Event {
        Event::Clock(id, 0)
    }

    pub fn reset(id: VarId) -> Event {
        Event::Reset(id, 0)
    }

    pub fn var_id(&self) -> Option<VarId> {
        match self {
            Event::Clock(id, _) | Event::Reset(id, _) if *id != VarId::SYNTHETIC => Some(*id),
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
