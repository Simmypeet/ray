//! What generated code may refer to, discovered while it is printed.

use rayc_mono_ir::{
    MonoFunctionInstance,
    function::MonoFunction,
    ty::{AggregateType, FunctionSignature, MonoType, Tuple},
};
use rayc_qbice::TrackedEngine;

use crate::{
    aggregates::AggregateRegistry,
    unit::TranslationUnit,
    worklist::{FunctionWorklist, PendingFunction},
};

/// The program-wide state shared by every function: the engine, the
/// functions still to generate, and the aggregate types used so far.
///
/// Everything else (signatures, names, handler layouts) is derived from
/// engine queries on demand, which the engine caches.
#[derive(Debug)]
pub(crate) struct Program<'engine> {
    engine: &'engine TrackedEngine,
    functions: FunctionWorklist,
    aggregates: AggregateRegistry,
    /// The empty tuple, which is the type of every unit constant.
    unit: AggregateType,
}

impl<'engine> Program<'engine> {
    pub(crate) fn new(engine: &'engine TrackedEngine, functions: FunctionWorklist) -> Self {
        let unit = AggregateType::Tuple(Tuple::new(engine.intern_unsized(Vec::new())));
        Self { engine, functions, aggregates: AggregateRegistry::default(), unit }
    }

    pub(crate) const fn engine(&self) -> &'engine TrackedEngine { self.engine }

    /// The type of every unit constant.
    pub(crate) const fn unit_type(&self) -> &AggregateType { &self.unit }

    /// Takes the next function to generate.
    pub(crate) fn next_function(&mut self) -> Option<PendingFunction> { self.functions.pop() }

    /// Whether `function` has ever been scheduled.
    pub(crate) fn has_scheduled(&self, function: &MonoFunctionInstance) -> bool {
        self.functions.contains(function)
    }

    /// Records that generated code refers to `ty`.
    pub(crate) async fn use_type(&mut self, ty: &MonoType) {
        self.aggregates.insert_type(self.engine, ty).await;
    }

    /// Records that generated code refers to every type in `signature`.
    pub(crate) async fn use_signature(&mut self, signature: &FunctionSignature) {
        self.aggregates.insert_signature(self.engine, signature).await;
    }

    /// Records that generated code constructs `aggregate`.
    pub(crate) async fn use_aggregate(&mut self, aggregate: AggregateType) {
        self.aggregates.insert(self.engine, aggregate).await;
    }

    /// Records that generated code calls or takes the address of the
    /// function, scheduling it.
    pub(crate) fn use_function(&mut self, instance: &MonoFunctionInstance) {
        self.functions.insert(instance.clone());
    }

    /// Adds every used aggregate to `unit`.
    pub(crate) fn define_aggregates(&self, unit: &mut TranslationUnit) {
        unit.define_aggregates(&self.aggregates);
    }

    /// Lowers the body of an internal function.
    pub(crate) async fn lower(&self, instance: MonoFunctionInstance) -> MonoFunction {
        rayc_mono_ir_builder::lower_function(self.engine, instance).await
    }
}
