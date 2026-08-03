//! Memory locations and byte-range aliasing.
//!
//! These types are one alias-domain implementation.  MemorySSA itself does
//! not depend on them: clients may use a different query and alias oracle.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MemoryLocation<O> {
    pub object: O,
    pub offset: i64,
    pub byte_len: usize,
}

impl<O: Copy> MemoryLocation<O> {
    #[must_use]
    pub fn end(self) -> Option<i64> {
        self.offset.checked_add(i64::try_from(self.byte_len).ok()?)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryEffect<O> {
    Exact(MemoryLocation<O>),
    UnknownObject(O),
    UnknownAll,
}

/// Conservatively decide whether two byte-range effects may touch the same
/// memory.  Callers are responsible for rejecting empty or overflowing exact
/// ranges when constructing their IR adapter.
#[must_use]
pub fn effects_may_alias<O: Copy + Eq>(left: MemoryEffect<O>, right: MemoryEffect<O>) -> bool {
    match (left, right) {
        (MemoryEffect::UnknownAll, _) | (_, MemoryEffect::UnknownAll) => true,
        (MemoryEffect::UnknownObject(left), MemoryEffect::UnknownObject(right)) => left == right,
        (MemoryEffect::UnknownObject(object), MemoryEffect::Exact(location))
        | (MemoryEffect::Exact(location), MemoryEffect::UnknownObject(object)) => {
            object == location.object
        }
        (MemoryEffect::Exact(left), MemoryEffect::Exact(right)) => {
            if left.object != right.object {
                return false;
            }
            let (Some(left_end), Some(right_end)) = (left.end(), right.end()) else {
                return true;
            };
            left.offset < right_end && right.offset < left_end
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exact(object: u8, offset: i64, byte_len: usize) -> MemoryEffect<u8> {
        MemoryEffect::Exact(MemoryLocation {
            object,
            offset,
            byte_len,
        })
    }

    #[test]
    fn exact_aliasing_is_object_and_half_open_range_based() {
        assert!(effects_may_alias(exact(1, 4, 8), exact(1, 8, 8)));
        assert!(!effects_may_alias(exact(1, 0, 8), exact(1, 8, 8)));
        assert!(!effects_may_alias(exact(1, 4, 8), exact(2, 4, 8)));
    }

    #[test]
    fn unknown_effects_are_conservative_within_their_domain() {
        assert!(effects_may_alias(
            MemoryEffect::UnknownObject(1),
            exact(1, 64, 8)
        ));
        assert!(!effects_may_alias(
            MemoryEffect::UnknownObject(1),
            exact(2, 64, 8)
        ));
        assert!(effects_may_alias(MemoryEffect::UnknownAll, exact(2, 64, 8)));
    }
}
