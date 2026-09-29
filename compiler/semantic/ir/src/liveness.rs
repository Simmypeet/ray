//! Backward liveness analyses of an IR function.
//!
//! A value is **use-live** at a point when some path from that point uses it,
//! and **drop-live** when its value is not used again on any path, but is
//! still dropped on some path. The two sets are disjoint: a value that is
//! use-live is not also reported as drop-live, since a use keeps everything
//! in its type alive anyway.
//!
//! The borrow checker needs the distinction because a drop keeps fewer
//! regions of the dropped type alive than a use does.
//!
//! - [`local`] computes the liveness of locals: variables and captures.
//! - [`expr`] computes the liveness of expression values.

use std::{collections::BTreeSet, convert::Infallible};

use crate::dataflow::JoinLattice;

pub mod expr;
pub mod local;

/// The values live at one point of a function, split into use-live and
/// drop-live ones. The two sets are disjoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSet<T> {
    use_live: BTreeSet<T>,
    drop_live: BTreeSet<T>,
}

impl<T> Default for LiveSet<T> {
    fn default() -> Self { Self { use_live: BTreeSet::new(), drop_live: BTreeSet::new() } }
}

impl<T: Ord + Copy> LiveSet<T> {
    /// Returns whether the current value of `value` may be used later.
    #[must_use]
    pub fn is_use_live(&self, value: T) -> bool { self.use_live.contains(&value) }

    /// Returns whether the current value of `value` may be dropped later
    /// without being used first.
    #[must_use]
    pub fn is_drop_live(&self, value: T) -> bool { self.drop_live.contains(&value) }

    /// Returns the use-live values, in order.
    #[must_use]
    pub fn use_live(&self) -> impl ExactSizeIterator<Item = T> + '_ {
        self.use_live.iter().copied()
    }

    /// Returns the drop-live values, in order.
    #[must_use]
    pub fn drop_live(&self) -> impl ExactSizeIterator<Item = T> + '_ {
        self.drop_live.iter().copied()
    }

    /// Records a use of `value`, which subsumes a later drop of it.
    pub(crate) fn mark_used(&mut self, value: T) {
        self.use_live.insert(value);
        self.drop_live.remove(&value);
    }

    /// Records a drop of `value`, unless it is used later anyway.
    pub(crate) fn mark_dropped(&mut self, value: T) {
        if !self.use_live.contains(&value) {
            self.drop_live.insert(value);
        }
    }

    /// Records a definition of `value`, before which its new value is dead.
    pub(crate) fn mark_defined(&mut self, value: T) {
        self.use_live.remove(&value);
        self.drop_live.remove(&value);
    }
}

impl<T, D> JoinLattice<D, Infallible> for LiveSet<T>
where
    T: Ord + Copy + Send + Sync,
    D: Sync + ?Sized,
{
    async fn join(&mut self, other: &Self, _dataflow_problem_ctx: &D) -> Result<bool, Infallible> {
        let before = self.clone();

        // A local used on any path is use-live, which subsumes being dropped
        // on another path.
        self.use_live.extend(other.use_live.iter().copied());
        self.drop_live.extend(other.drop_live.iter().copied());
        self.drop_live.retain(|value| !self.use_live.contains(value));

        Ok(*self != before)
    }
}
