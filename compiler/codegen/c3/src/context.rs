//! The state of generating one function.

use rayc_mono_ir::{MonoFunctionInstance, ty::FunctionSignature};
use rayc_qbice::TrackedEngine;

use crate::{aggregates::AggregateRegistry, function::FunctionOutput};

/// What generating one function has found so far: the functions and the
/// aggregate types its code refers to.
///
/// Each function owns its context, so no state is shared with any other
/// function being generated. The rule it enforces is "register before you
/// spell": C syntax is rendered synchronously and cannot query the engine, so
/// every type is registered, with an `.await`, before it is written.
#[derive(Debug)]
pub(crate) struct FunctionContext<'engine> {
    engine: &'engine TrackedEngine,
    aggregates: AggregateRegistry,
    callees: Vec<MonoFunctionInstance>,
}

impl<'engine> FunctionContext<'engine> {
    pub(crate) fn new(engine: &'engine TrackedEngine) -> Self {
        Self { engine, aggregates: AggregateRegistry::default(), callees: Vec::new() }
    }

    /// The facts about the registered aggregates that spelling a type needs.
    pub(crate) const fn aggregate_registry(&self) -> &AggregateRegistry { &self.aggregates }

    /// Records that the generated code refers to every type in `signature`.
    pub(crate) async fn use_signature(&mut self, signature: &FunctionSignature) {
        self.aggregates.insert_signature(self.engine, signature).await;
    }

    /// Records that the generated code calls or takes the address of
    /// `instance`, which must be generated too.
    pub(crate) fn use_function(&mut self, instance: &MonoFunctionInstance) {
        self.callees.push(instance.clone());
    }

    /// Finishes generating `instance`, declared as `declaration`.
    pub(crate) fn finish(
        self,
        instance: MonoFunctionInstance,
        declaration: String,
    ) -> FunctionOutput {
        FunctionOutput::new(
            instance,
            declaration,
            self.callees,
            self.aggregates.into_aggregates().collect(),
        )
    }
}
