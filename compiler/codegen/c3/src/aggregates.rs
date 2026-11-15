//! The aggregate types a function's generated code spells.
//!
//! Every function has a registry of its own, which only collects what the
//! function spells, as it spells it. The aggregates those contain are found
//! once for the whole program, after every function is generated, so a type
//! used by many functions is walked only once.

use rayc_hash::FxHashSet;
use rayc_mono_ir::ty::AggregateType;

/// Every aggregate type one function's generated code spells, in no
/// particular order.
#[derive(Debug, Default)]
pub(crate) struct AggregateRegistry {
    found: FxHashSet<AggregateType>,
}

impl AggregateRegistry {
    /// Records `aggregate`, unless it was recorded before.
    pub(crate) fn insert(&mut self, aggregate: &AggregateType) {
        // check before cloning: most aggregates are spelled more than once
        if !self.found.contains(aggregate) {
            self.found.insert(aggregate.clone());
        }
    }

    /// Every aggregate recorded, in no particular order.
    pub(crate) fn into_aggregates(self) -> impl Iterator<Item = AggregateType> {
        self.found.into_iter()
    }
}
