//! The loop that discovers and generates every reachable function.

use std::collections::{BTreeMap, VecDeque};

use rayc_hash::FxHashSet;
use rayc_mono_ir::MonoFunctionInstance;
use rayc_qbice::TrackedEngine;

use crate::function::{FunctionOutput, generate_function};

/// The generated functions, ordered by instance.
///
/// The order depends only on which functions were generated, never on the
/// order they were generated in, so the output stays the same once functions
/// are generated in parallel.
pub(crate) type GeneratedFunctions = BTreeMap<MonoFunctionInstance, FunctionOutput>;

/// Generates every function reachable from a set of roots.
///
/// The coordinator alone decides which functions still need generating: it
/// owns the set of functions ever scheduled, and each generated function only
/// reports the functions it refers to.
#[derive(Debug)]
pub(crate) struct Coordinator<'engine> {
    engine: &'engine TrackedEngine,
    /// Every function ever scheduled, generated or not.
    seen: FxHashSet<MonoFunctionInstance>,
    /// Scheduled functions not generated yet, first in first out.
    pending: VecDeque<MonoFunctionInstance>,
    outputs: GeneratedFunctions,
}

impl<'engine> Coordinator<'engine> {
    pub(crate) fn new(engine: &'engine TrackedEngine) -> Self {
        Self {
            engine,
            seen: FxHashSet::default(),
            pending: VecDeque::new(),
            outputs: BTreeMap::new(),
        }
    }

    /// Generates `roots` and every function reachable from them.
    pub(crate) async fn generate(
        mut self,
        roots: impl IntoIterator<Item = impl Into<MonoFunctionInstance>>,
    ) -> GeneratedFunctions {
        // roots are scheduled in a canonical order, so the order functions are
        // generated in does not depend on how the caller listed them
        let mut roots = roots.into_iter().map(Into::into).collect::<Vec<_>>();
        roots.sort_unstable();
        for root in roots {
            self.schedule(root);
        }

        // generating a function may schedule more; the loop ends once every
        // scheduled function is generated, which happens since each is
        // scheduled at most once and finitely many are reachable
        while let Some(instance) = self.pending.pop_front() {
            let output = generate_function(self.engine, instance).await;
            for callee in output.callees() {
                self.schedule(callee.clone());
            }
            self.outputs.insert(output.instance().clone(), output);
        }

        self.outputs
    }

    /// Schedules `instance` unless it has been scheduled before.
    fn schedule(&mut self, instance: MonoFunctionInstance) {
        if self.seen.insert(instance.clone()) {
            self.pending.push_back(instance);
        }
    }
}
