//! Unit-independent exact interval indexing.
//!
//! The index is deliberately separate from the byte-oriented memory adapter.
//! Clients may use bytes, bits, words, or another ordered unit without lying
//! about the alias domain.  Construction costs `O(N log N)` time and `O(N)`
//! space.  One overlap query costs `O(log N + K)`, where `K` is the number of
//! definitions returned; neither bound depends on the numerical interval
//! width.

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExactInterval<O, V> {
    pub object: O,
    pub start: usize,
    pub length: usize,
    pub value: V,
}

impl<O, V> ExactInterval<O, V> {
    #[must_use]
    pub fn end(&self) -> Option<usize> {
        self.start.checked_add(self.length)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisjointIntervalError<V> {
    Empty { value: V },
    Overflow { value: V },
    Overlap { first: V, second: V },
}

#[derive(Debug, Clone, Copy)]
struct Entry<V> {
    start: usize,
    end: usize,
    value: V,
}

#[derive(Debug)]
pub struct DisjointIntervalMap<O, V> {
    objects: BTreeMap<O, Vec<Entry<V>>>,
}

impl<O: Ord, V: Copy> DisjointIntervalMap<O, V> {
    pub fn try_new(
        intervals: impl IntoIterator<Item = ExactInterval<O, V>>,
    ) -> Result<Self, DisjointIntervalError<V>> {
        let mut objects = BTreeMap::<O, Vec<Entry<V>>>::new();
        for interval in intervals {
            if interval.length == 0 {
                return Err(DisjointIntervalError::Empty {
                    value: interval.value,
                });
            }
            let Some(end) = interval.end() else {
                return Err(DisjointIntervalError::Overflow {
                    value: interval.value,
                });
            };
            objects.entry(interval.object).or_default().push(Entry {
                start: interval.start,
                end,
                value: interval.value,
            });
        }

        for entries in objects.values_mut() {
            entries.sort_unstable_by_key(|entry| entry.start);
            for pair in entries.windows(2) {
                if pair[0].end > pair[1].start {
                    return Err(DisjointIntervalError::Overlap {
                        first: pair[0].value,
                        second: pair[1].value,
                    });
                }
            }
        }
        Ok(Self { objects })
    }

    pub fn overlapping(
        &self,
        object: &O,
        start: usize,
        length: usize,
    ) -> Result<Overlapping<'_, V>, InvalidInterval> {
        if length == 0 {
            return Err(InvalidInterval::Empty);
        }
        let end = start.checked_add(length).ok_or(InvalidInterval::Overflow)?;
        let entries = self.objects.get(object).map_or(&[][..], Vec::as_slice);
        let cursor = entries.partition_point(|entry| entry.end <= start);
        Ok(Overlapping {
            entries,
            cursor,
            end,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidInterval {
    Empty,
    Overflow,
}

pub struct Overlapping<'a, V> {
    entries: &'a [Entry<V>],
    cursor: usize,
    end: usize,
}

impl<V: Copy> Iterator for Overlapping<'_, V> {
    type Item = V;

    fn next(&mut self) -> Option<Self::Item> {
        let entry = self.entries.get(self.cursor)?;
        if entry.start >= self.end {
            return None;
        }
        self.cursor += 1;
        Some(entry.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_queries_return_only_overlapping_disjoint_definitions() {
        let index = DisjointIntervalMap::try_new([
            ExactInterval {
                object: 1u8,
                start: 0,
                length: 8,
                value: 10,
            },
            ExactInterval {
                object: 1,
                start: 16,
                length: 8,
                value: 11,
            },
            ExactInterval {
                object: 2,
                start: 4,
                length: 8,
                value: 12,
            },
        ])
        .unwrap();

        assert_eq!(
            index.overlapping(&1, 4, 16).unwrap().collect::<Vec<_>>(),
            vec![10, 11]
        );
        assert_eq!(
            index.overlapping(&1, 8, 8).unwrap().collect::<Vec<_>>(),
            Vec::<i32>::new()
        );
        assert_eq!(
            index.overlapping(&2, 0, 4).unwrap().collect::<Vec<_>>(),
            Vec::<i32>::new()
        );
    }

    #[test]
    fn construction_rejects_overlapping_definitions() {
        let error = DisjointIntervalMap::try_new([
            ExactInterval {
                object: 1u8,
                start: 8,
                length: 8,
                value: 3,
            },
            ExactInterval {
                object: 1,
                start: 0,
                length: 9,
                value: 2,
            },
        ])
        .unwrap_err();

        assert_eq!(
            error,
            DisjointIntervalError::Overlap {
                first: 2,
                second: 3
            }
        );
    }

    #[test]
    fn numerical_width_does_not_expand_the_index() {
        let index = DisjointIntervalMap::try_new([ExactInterval {
            object: 1u8,
            start: 0,
            length: 16 * 1024 * 1024,
            value: 7,
        }])
        .unwrap();

        assert_eq!(
            index
                .overlapping(&1, 8 * 1024 * 1024, 1)
                .unwrap()
                .collect::<Vec<_>>(),
            vec![7]
        );
    }
}
