//! Backward liveness analyses of an IR function.
//!
//! A value is **use-live** at a point when some path from that point uses it,
//! and **drop-live** when its value is not used again on any path, but is
//! still dropped by a non-no-op `Drop` instance on some path. Each value has
//! one live mode: a use subsumes a drop, since a use keeps everything in its
//! type alive anyway.
//!
//! The borrow checker needs the distinction because a drop keeps fewer
//! regions of the dropped type alive than a use does.
//!
//! - [`local`] computes the liveness of locals: variables and captures.
//! - [`expr`] computes the liveness of expression values.

use std::{
    collections::{BTreeMap, btree_map::Entry},
    convert::Infallible,
};

use crate::dataflow::JoinLattice;

pub mod expr;
pub mod local;

/// The reason a value is live at one point of a function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveMode {
    /// The value is only dropped later by a non-no-op `Drop` instance.
    Drop,
    /// The value may be used later, which subsumes a later drop.
    Use,
}

/// The values live at one point of a function and their live modes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSet<T> {
    live: BTreeMap<T, LiveMode>,
}

impl<T> Default for LiveSet<T> {
    fn default() -> Self { Self { live: BTreeMap::new() } }
}

impl<T: Ord + Copy> LiveSet<T> {
    /// Returns whether the current value of `value` may be used later.
    #[must_use]
    pub fn is_use_live(&self, value: T) -> bool { self.live.get(&value) == Some(&LiveMode::Use) }

    /// Returns whether the current value of `value` may be dropped later
    /// without being used first.
    #[must_use]
    pub fn is_drop_live(&self, value: T) -> bool { self.live.get(&value) == Some(&LiveMode::Drop) }

    /// Returns the use-live values, in order.
    pub fn use_live(&self) -> impl Iterator<Item = T> + '_ {
        self.live.iter().filter_map(|(&value, &mode)| (mode == LiveMode::Use).then_some(value))
    }

    /// Returns the drop-live values, in order.
    pub fn drop_live(&self) -> impl Iterator<Item = T> + '_ {
        self.live.iter().filter_map(|(&value, &mode)| (mode == LiveMode::Drop).then_some(value))
    }

    /// Records a use of `value`, which subsumes a later drop of it.
    pub(crate) fn mark_used(&mut self, value: T) { self.live.insert(value, LiveMode::Use); }

    /// Records a drop of `value`, unless it is used later anyway.
    pub(crate) fn mark_dropped(&mut self, value: T) {
        self.live.entry(value).or_insert(LiveMode::Drop);
    }

    /// Records a definition of `value`, before which its new value is dead.
    pub(crate) fn mark_defined(&mut self, value: T) { self.live.remove(&value); }
}

impl<T, D> JoinLattice<D, Infallible> for LiveSet<T>
where
    T: Ord + Copy + Send + Sync,
    D: Sync + ?Sized,
{
    async fn join(&mut self, other: &Self, _dataflow_problem_ctx: &D) -> Result<bool, Infallible> {
        let mut changed = false;

        // A local used on any path is use-live, which subsumes being dropped
        // on another path.
        for (&value, &mode) in &other.live {
            match self.live.entry(value) {
                Entry::Vacant(entry) => {
                    entry.insert(mode);
                    changed = true;
                }
                Entry::Occupied(mut entry) => {
                    if *entry.get() == LiveMode::Drop && mode == LiveMode::Use {
                        entry.insert(LiveMode::Use);
                        changed = true;
                    }
                }
            }
        }

        Ok(changed)
    }
}
