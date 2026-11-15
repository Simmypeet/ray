//! What generated code may refer to, discovered while it is printed.

use qbice::storage::intern::Interned;
use rayc_mono_ir::{
    MonoFunctionInstance,
    function::{MonoFunction, get_mono_function},
    operand::{Constant, Operand},
    place::Place,
    ty::{AggregateType, FunctionSignature, MonoType},
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
}

impl<'engine> Program<'engine> {
    pub(crate) fn new(engine: &'engine TrackedEngine, functions: FunctionWorklist) -> Self {
        Self { engine, functions, aggregates: AggregateRegistry::default() }
    }

    pub(crate) const fn engine(&self) -> &'engine TrackedEngine { self.engine }

    /// The layouts of the aggregates used so far, which spelling a type
    /// depends on.
    pub(crate) const fn layouts(&self) -> &AggregateRegistry { &self.aggregates }

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

    /// Whether a value of `ty` holds nothing, recording `ty`.
    pub(crate) async fn is_zero_sized(&mut self, ty: &MonoType) -> bool {
        self.use_type(ty).await;
        self.aggregates.is_zero_sized(ty)
    }

    /// The type of `place` in `function`, all of whose locals' types have been
    /// recorded.
    pub(crate) fn place_type(&self, function: &MonoFunction, place: &Place) -> Interned<MonoType> {
        let mut ty = function.get_local(place.local()).ty().clone();
        for projection in place.projections() {
            ty = self.aggregates.projected_type(&ty, *projection, self.engine);
        }
        ty
    }

    /// Whether `place` in `function` holds nothing.
    pub(crate) async fn is_place_zero_sized(
        &mut self,
        function: &MonoFunction,
        place: &Place,
    ) -> bool {
        let ty = self.place_type(function, place);
        self.is_zero_sized(&ty).await
    }

    /// Whether the value of `operand` in `function` holds nothing.
    pub(crate) async fn is_operand_zero_sized(
        &mut self,
        function: &MonoFunction,
        operand: &Operand,
    ) -> bool {
        match operand {
            Operand::Copy(place) => self.is_place_zero_sized(function, place).await,
            Operand::Constant(constant) => self.is_constant_zero_sized(constant).await,
            Operand::Function(_) => false,
        }
    }

    /// Whether `constant` holds nothing.
    async fn is_constant_zero_sized(&mut self, constant: &Constant) -> bool {
        match constant {
            Constant::Unit => true,
            Constant::EmptyAggregate(aggregate) => {
                self.use_aggregate(aggregate.clone()).await;
                self.aggregates.is_aggregate_zero_sized(aggregate)
            }
            Constant::Bool(_)
            | Constant::Int8(_)
            | Constant::Int16(_)
            | Constant::Int32(_)
            | Constant::Int64(_)
            | Constant::Isize(_)
            | Constant::Uint8(_)
            | Constant::Uint16(_)
            | Constant::Uint32(_)
            | Constant::Uint64(_)
            | Constant::Usize(_)
            | Constant::Float32(_)
            | Constant::CInt(_)
            | Constant::CStr(_)
            | Constant::NullPointer(_) => false,
        }
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

    /// The body of an internal function, which the engine lowers once and
    /// caches.
    pub(crate) async fn lower(&self, instance: MonoFunctionInstance) -> Interned<MonoFunction> {
        self.engine.get_mono_function(instance).await
    }
}
