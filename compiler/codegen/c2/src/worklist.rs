//! The queue of `MonoIR` fragments awaiting code generation.

use std::collections::VecDeque;

use rayc_hash::{FxHashMap, FxHashSet};
use rayc_mono_ir::{MonoDefInstance, MonoFragmentInstance, MonoIR};

/// A fragment taken from the [`FragmentWorklist`].
#[derive(Debug)]
pub(crate) enum PendingFragment {
    /// The caller supplied the fragment's lowered body up front.
    Lowered(MonoIR),
    /// The fragment must still be lowered (or, for an extern definition,
    /// only declared).
    Unlowered(MonoFragmentInstance),
}

/// A first-in, first-out queue of fragments that yields each fragment once.
#[derive(Debug, Default)]
pub(crate) struct FragmentWorklist {
    pending: VecDeque<MonoFragmentInstance>,
    seen: FxHashSet<MonoFragmentInstance>,
    preloaded: FxHashMap<MonoFragmentInstance, MonoIR>,
}

impl FragmentWorklist {
    /// Creates a worklist whose fragments, when reached, are taken from
    /// `preloaded` instead of being lowered.
    pub(crate) fn with_preloaded(preloaded: impl IntoIterator<Item = MonoIR>) -> Self {
        let mut worklist = Self::default();
        for ir in preloaded {
            let previous = worklist.preloaded.insert(ir.instance().clone(), ir);
            assert!(
                previous.is_none(),
                "a MonoIR definition instance should only be preloaded once"
            );
        }
        worklist
    }

    /// The source definitions whose bodies were supplied up front.
    pub(crate) fn preloaded_definitions(&self) -> impl Iterator<Item = &MonoDefInstance> {
        self.preloaded.keys().filter_map(|fragment| match fragment {
            MonoFragmentInstance::Definition(instance) => Some(instance),
            MonoFragmentInstance::NominalDrop(_) => None,
        })
    }

    /// Schedules `fragment` unless it has been scheduled before.
    pub(crate) fn insert(&mut self, fragment: impl Into<MonoFragmentInstance>) {
        let fragment = fragment.into();
        if self.seen.insert(fragment.clone()) {
            self.pending.push_back(fragment);
        }
    }

    /// Whether `fragment` has ever been scheduled.
    pub(crate) fn contains(&self, fragment: &MonoFragmentInstance) -> bool {
        self.seen.contains(fragment)
    }

    /// Takes the next scheduled fragment.
    pub(crate) fn pop(&mut self) -> Option<PendingFragment> {
        let fragment = self.pending.pop_front()?;
        Some(
            self.preloaded
                .remove(&fragment)
                .map_or(PendingFragment::Unlowered(fragment), PendingFragment::Lowered),
        )
    }
}
