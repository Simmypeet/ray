//! The queue of `MonoFunction`s awaiting code generation.

use std::collections::VecDeque;

use rayc_hash::{FxHashMap, FxHashSet};
use rayc_mono_ir::{MonoFunctionInstance, function::MonoFunction};

/// A function taken from the [`FunctionWorklist`].
#[derive(Debug)]
pub(crate) enum PendingFunction {
    /// The caller supplied the function's lowered body up front.
    Lowered(Box<MonoFunction>),
    /// The function must still be lowered (or, for an `extern def`, only
    /// declared).
    Unlowered(MonoFunctionInstance),
}

/// A first-in, first-out queue of functions that yields each function once.
#[derive(Debug, Default)]
pub(crate) struct FunctionWorklist {
    pending: VecDeque<MonoFunctionInstance>,
    seen: FxHashSet<MonoFunctionInstance>,
    preloaded: FxHashMap<MonoFunctionInstance, MonoFunction>,
}

impl FunctionWorklist {
    /// Creates a worklist whose functions, when reached, are taken from
    /// `preloaded` instead of being lowered.
    pub(crate) fn with_preloaded(preloaded: impl IntoIterator<Item = MonoFunction>) -> Self {
        let mut worklist = Self::default();
        for function in preloaded {
            let previous = worklist.preloaded.insert(function.instance().clone(), function);
            assert!(previous.is_none(), "a MonoFunction should only be preloaded once");
        }
        worklist
    }

    /// Schedules `function` unless it has been scheduled before.
    pub(crate) fn insert(&mut self, function: impl Into<MonoFunctionInstance>) {
        let function = function.into();
        if self.seen.insert(function.clone()) {
            self.pending.push_back(function);
        }
    }

    /// Whether `function` has ever been scheduled.
    pub(crate) fn contains(&self, function: &MonoFunctionInstance) -> bool {
        self.seen.contains(function)
    }

    /// Takes the next scheduled function.
    pub(crate) fn pop(&mut self) -> Option<PendingFunction> {
        let function = self.pending.pop_front()?;
        Some(
            self.preloaded
                .remove(&function)
                .map_or(PendingFunction::Unlowered(function), |function| {
                    PendingFunction::Lowered(Box::new(function))
                }),
        )
    }
}
