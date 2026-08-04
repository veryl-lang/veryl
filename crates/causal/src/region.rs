//! Width-independent region identities and conservative aliasing.

/// A half-open interval measured in a client-selected unit (normally bits).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Span {
    pub start: usize,
    pub length: usize,
}

impl Span {
    #[must_use]
    pub fn end(self) -> Option<usize> {
        self.start.checked_add(self.length)
    }

    #[must_use]
    pub fn intersection(self, other: Self) -> Option<Self> {
        let start = self.start.max(other.start);
        let end = self.end()?.min(other.end()?);
        if start < end {
            Some(Self {
                start,
                length: end - start,
            })
        } else {
            None
        }
    }
}

/// A statically resolved region, or an unresolved access within one object.
///
/// `UnknownObject` is intentionally not expanded to the object's numerical
/// width in this value. Clients with a declared object span can expand it
/// conservatively and still produce a complete result; without that span the
/// access remains incomplete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Region<O> {
    Exact {
        object: O,
        span: Span,
    },
    /// An unresolved suffix confined to a statically known prefix span.
    UnknownRegion {
        object: O,
        span: Span,
    },
    UnknownObject(O),
    UnknownAll,
}

impl<O: Copy + Eq> Region<O> {
    #[must_use]
    pub fn may_alias(self, other: Self) -> bool {
        match (self, other) {
            (Self::UnknownAll, _) | (_, Self::UnknownAll) => true,
            (
                Self::UnknownRegion {
                    object: left,
                    span: left_span,
                },
                Self::UnknownRegion {
                    object: right,
                    span: right_span,
                },
            ) => left == right && left_span.intersection(right_span).is_some(),
            (
                Self::UnknownRegion { object, span },
                Self::Exact {
                    object: other,
                    span: other_span,
                },
            )
            | (
                Self::Exact {
                    object: other,
                    span: other_span,
                },
                Self::UnknownRegion { object, span },
            ) => object == other && span.intersection(other_span).is_some(),
            (Self::UnknownObject(left), Self::UnknownObject(right)) => left == right,
            (Self::UnknownObject(object), Self::UnknownRegion { object: other, .. })
            | (Self::UnknownRegion { object: other, .. }, Self::UnknownObject(object)) => {
                object == other
            }
            (Self::UnknownObject(object), Self::Exact { object: other, .. })
            | (Self::Exact { object: other, .. }, Self::UnknownObject(object)) => object == other,
            (
                Self::Exact {
                    object: left,
                    span: left_span,
                },
                Self::Exact {
                    object: right,
                    span: right_span,
                },
            ) => left == right && left_span.intersection(right_span).is_some(),
        }
    }

    #[must_use]
    pub fn is_exact(self) -> bool {
        matches!(self, Self::Exact { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exact(object: u8, start: usize, length: usize) -> Region<u8> {
        Region::Exact {
            object,
            span: Span { start, length },
        }
    }

    #[test]
    fn exact_regions_use_half_open_intervals() {
        assert!(exact(1, 3, 5).may_alias(exact(1, 7, 2)));
        assert!(!exact(1, 3, 5).may_alias(exact(1, 8, 2)));
        assert!(!exact(1, 3, 5).may_alias(exact(2, 3, 5)));
    }

    #[test]
    fn unknown_object_does_not_become_unknown_all() {
        assert!(Region::UnknownObject(1).may_alias(exact(1, 1 << 30, 1)));
        assert!(!Region::UnknownObject(1).may_alias(exact(2, 0, 1)));
    }

    #[test]
    fn unknown_region_aliases_only_its_static_prefix() {
        let prefix = Region::UnknownRegion {
            object: 1,
            span: Span {
                start: 8,
                length: 8,
            },
        };
        assert!(prefix.may_alias(exact(1, 12, 1)));
        assert!(!prefix.may_alias(exact(1, 0, 8)));
        assert!(!prefix.may_alias(exact(2, 12, 1)));
    }
}
