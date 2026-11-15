//! The state of generating one function.

use qbice::storage::intern::Interned;
use rayc_mono_ir::{
    MonoFunctionInstance,
    function::MonoFunction,
    operand::{Constant, Operand},
    place::Place,
    ty::{AggregateType, FunctionSignature, MonoType},
};
use rayc_qbice::TrackedEngine;

use crate::{aggregates::AggregateRegistry, function::FunctionOutput, projection::projected_type};

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

    pub(crate) const fn engine(&self) -> &'engine TrackedEngine { self.engine }

    /// The facts about the registered aggregates that spelling a type needs.
    pub(crate) const fn aggregate_registry(&self) -> &AggregateRegistry { &self.aggregates }

    /// Records that the generated code refers to `ty`.
    pub(crate) async fn use_type(&mut self, ty: &MonoType) {
        self.aggregates.insert_type(self.engine, ty).await;
    }

    /// Records that the generated code refers to every type in `signature`.
    pub(crate) async fn use_signature(&mut self, signature: &FunctionSignature) {
        self.aggregates.insert_signature(self.engine, signature).await;
    }

    /// Records that the generated code constructs `aggregate`.
    pub(crate) async fn use_aggregate(&mut self, aggregate: AggregateType) {
        self.aggregates.insert(self.engine, aggregate).await;
    }

    /// Records that the generated code calls or takes the address of
    /// `instance`, which must be generated too.
    pub(crate) fn use_function(&mut self, instance: &MonoFunctionInstance) {
        self.callees.push(instance.clone());
    }

    /// Whether a value of `ty` holds nothing, recording `ty`.
    pub(crate) async fn is_zero_sized(&mut self, ty: &MonoType) -> bool {
        self.use_type(ty).await;
        self.aggregates.is_zero_sized(ty)
    }

    /// The type of `place` in `function`: the type of its local, narrowed by
    /// each projection in turn.
    pub(crate) async fn place_type(
        &self,
        function: &MonoFunction,
        place: &Place,
    ) -> Interned<MonoType> {
        let mut ty = function.get_local(place.local()).ty().clone();
        for projection in place.projections() {
            ty = projected_type(self.engine, &ty, *projection).await;
        }
        ty
    }

    /// Whether `place` in `function` holds nothing, recording its type.
    pub(crate) async fn is_place_zero_sized(
        &mut self,
        function: &MonoFunction,
        place: &Place,
    ) -> bool {
        let ty = self.place_type(function, place).await;
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
            // a function is spelled as its address
            Operand::Function(_) => false,
        }
    }

    /// Whether `constant` holds nothing, recording its type.
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

    /// Finishes generating `instance`, described as `description`, declared
    /// as `declaration` and defined as `definition` unless the C environment
    /// provides it.
    pub(crate) fn finish(
        self,
        instance: MonoFunctionInstance,
        description: String,
        declaration: String,
        definition: Option<String>,
    ) -> FunctionOutput {
        FunctionOutput::new(
            instance,
            description,
            declaration,
            definition,
            self.callees,
            self.aggregates.into_aggregates().collect(),
        )
    }
}
