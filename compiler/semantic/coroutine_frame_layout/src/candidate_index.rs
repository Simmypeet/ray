//! Numbers the values of a coroutine that may share memory with a value of
//! another variant, so that sets of them can be bitsets.
//!
//! See step A of `COROUTINE_FRAME_CONFLICT_PLAN.md`.

use rayc_hash::FxHashMap;
use rayc_transitive_closure::BitSet;

use crate::savable::Savable;

/// The values of one instance of a coroutine that are placed in exactly one
/// suspension variant, numbered densely from zero.
///
/// The numbers follow the order of [`Savable`], so visiting the values by
/// ascending number visits them in a deterministic order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateIndex {
    /// The values, sorted. A value's number is its position.
    values: Vec<Savable>,
    numbers: FxHashMap<Savable, usize>,
}

impl CandidateIndex {
    /// Numbers `values` in sorted order, each once.
    pub(crate) fn new(values: impl IntoIterator<Item = Savable>) -> Self {
        let mut values = values.into_iter().collect::<Vec<_>>();
        values.sort_unstable();
        values.dedup();

        let numbers = values.iter().enumerate().map(|(number, value)| (*value, number)).collect();

        Self { values, numbers }
    }

    /// Returns the number of candidates.
    #[must_use]
    pub const fn len(&self) -> usize { self.values.len() }

    /// Returns whether there is no candidate.
    #[must_use]
    pub const fn is_empty(&self) -> bool { self.values.is_empty() }

    /// Returns the number of `value`, or `None` when it is not a candidate.
    #[must_use]
    pub fn number_of(&self, value: Savable) -> Option<usize> { self.numbers.get(&value).copied() }

    /// Returns the value numbered `number`.
    ///
    /// # Panics
    ///
    /// Panics if `number` is not below [`Self::len`].
    #[must_use]
    pub fn value_of(&self, number: usize) -> Savable { self.values[number] }

    /// Iterates over the candidates, in ascending order of their numbers.
    #[must_use]
    pub fn values(&self) -> impl ExactSizeIterator<Item = Savable> + '_ {
        self.values.iter().copied()
    }

    /// Returns an empty set of candidates: a bitset with one bit per
    /// candidate.
    #[must_use]
    pub fn empty_set(&self) -> BitSet { BitSet::new(self.values.len()) }

    /// Returns the set of the candidates among `values`. The other values
    /// are ignored.
    #[must_use]
    pub fn set_of(&self, values: impl IntoIterator<Item = Savable>) -> BitSet {
        let mut set = self.empty_set();
        self.extend_set(&mut set, values);
        set
    }

    /// Adds the candidates among `values` to `set`, a set of this index. The
    /// other values are ignored.
    pub fn extend_set(&self, set: &mut BitSet, values: impl IntoIterator<Item = Savable>) {
        for value in values {
            if let Some(number) = self.number_of(value) {
                set.set(number);
            }
        }
    }
}
