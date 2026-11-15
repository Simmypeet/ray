//! Discovery of the aggregate types a function's generated code uses.
//!
//! Every function has a registry of its own. Discovering an aggregate in one
//! function never depends on another function, so functions can be generated
//! independently; the finished registries are merged once every function is
//! generated. Discovering the same aggregate in several functions repeats only
//! map lookups, since the engine caches every layout query.

use rayc_hash::{FxHashMap, FxHashSet};
use rayc_mono_ir::{
    MonoEffectInstance,
    layout::{
        build_environment_layout, build_handler_layout, build_struct_layout,
        coroutine::{build_effect_row_layout, build_frame_field_types},
        repr::{Repr, get_aggregate_repr},
        run_with::{RunWithClause, RunWithLayout, build_run_with_layout},
    },
    ty::{
        AggregateType, CoroutineYield, EffectHandlerCallback, EffectRow, FunctionSignature,
        MonoType, ReturnType,
    },
};
use rayc_qbice::TrackedEngine;

/// Every aggregate type that one function's generated code mentions, closed
/// under the types of their members.
///
/// The members matter because defining an aggregate in C spells the types of
/// its members, so those must be defined too.
///
/// Each insertion completes before returning: newly found aggregates have
/// their members, and their members' members, registered as part of the same
/// call.
#[derive(Debug, Default)]
pub(crate) struct AggregateRegistry {
    /// Every aggregate found so far.
    found: FxHashSet<AggregateType>,
    /// How each found aggregate is represented, which tells whether it is
    /// zero-sized.
    reprs: FxHashMap<AggregateType, Repr>,
}

impl AggregateRegistry {
    /// Registers every aggregate that `ty` mentions.
    pub(crate) async fn insert_type(&mut self, engine: &TrackedEngine, ty: &MonoType) {
        let mut pending = Vec::new();
        self.discover_type(ty, &mut pending);
        self.complete(engine, pending).await;
    }

    /// Registers `aggregate` itself.
    pub(crate) async fn insert(&mut self, engine: &TrackedEngine, aggregate: AggregateType) {
        let mut pending = Vec::new();
        self.discover(aggregate, &mut pending);
        self.complete(engine, pending).await;
    }

    /// Registers every aggregate that `signature` mentions.
    pub(crate) async fn insert_signature(
        &mut self,
        engine: &TrackedEngine,
        signature: &FunctionSignature,
    ) {
        let mut pending = Vec::new();
        self.discover_signature(signature, &mut pending);
        self.complete(engine, pending).await;
    }

    /// Whether a value of `ty` holds nothing, so it is never stored, passed
    /// or returned.
    ///
    /// # Panics
    ///
    /// Panics if `ty` is an aggregate that has not been registered: every type
    /// is registered before it is spelled.
    pub(crate) fn is_zero_sized(&self, ty: &MonoType) -> bool {
        ty.as_aggregate().is_some_and(|aggregate| self.is_aggregate_zero_sized(aggregate))
    }

    /// Whether a value of `aggregate` holds nothing. A zero-sized aggregate is
    /// only declared, never defined.
    ///
    /// # Panics
    ///
    /// Panics if `aggregate` has not been registered.
    pub(crate) fn is_aggregate_zero_sized(&self, aggregate: &AggregateType) -> bool {
        self.reprs
            .get(aggregate)
            .expect("an aggregate should be registered before it is spelled")
            .is_zero_sized()
    }

    /// Every aggregate found, in no particular order.
    pub(crate) fn into_aggregates(self) -> impl Iterator<Item = AggregateType> {
        self.found.into_iter()
    }

    /// Fetches the representation of each `pending` aggregate and discovers
    /// the aggregates its members mention, which may be pending in turn.
    ///
    /// `pending` is a local worklist rather than async recursion, which would
    /// need a heap-allocated future per level.
    async fn complete(&mut self, engine: &TrackedEngine, mut pending: Vec<AggregateType>) {
        while let Some(aggregate) = pending.pop() {
            let repr = engine.get_aggregate_repr(aggregate.clone()).await;
            self.reprs.insert(aggregate.clone(), repr);
            self.discover_members(engine, &aggregate, &mut pending).await;
        }
    }

    /// Discovers the aggregates that the members of `aggregate` mention,
    /// asking the layout query of its kind what the members are.
    async fn discover_members(
        &mut self,
        engine: &TrackedEngine,
        aggregate: &AggregateType,
        pending: &mut Vec<AggregateType>,
    ) {
        match aggregate {
            // one environment pointer and one function pointer per operation
            AggregateType::EffectHandlerCallback(handler) => {
                let instance = handler.mono_effect_instance().clone();
                let layout = engine.build_handler_layout(instance).await;
                for operation in layout.operations() {
                    self.discover_signature(operation.signature(), pending);
                }
            }
            AggregateType::Tuple(tuple) => {
                tuple.fields().iter().for_each(|field| self.discover_type(field, pending));
            }
            // the captured values, and a pointer to each handler record
            AggregateType::Environment(instance) => {
                let layout = engine.build_environment_layout(instance.clone()).await;
                for (_, capture) in layout.captures() {
                    self.discover_type(capture, pending);
                }
                for (_, record) in layout.handler_records() {
                    self.discover_handler_record(record.effect(), pending);
                }
            }
            AggregateType::Struct(instance) => {
                let layout = engine.build_struct_layout(instance.clone()).await;
                layout.fields().values().for_each(|field| self.discover_type(field, pending));
            }
            // every value saved across a suspension
            AggregateType::CoroutineFrame(instance) => {
                let types = engine.build_frame_field_types(instance.clone()).await;
                types.iter().for_each(|field| self.discover_type(field, pending));
            }
            AggregateType::CoroutineResume(resume) => {
                self.discover_row(engine, resume.row(), pending).await;
            }
            AggregateType::CoroutineYield(yielded) => {
                self.discover_row(engine, yielded.row(), pending).await;
            }
            // either the yielded operation of its row or the returned value
            AggregateType::CoroutineProgress(progress) => {
                let yielded =
                    AggregateType::CoroutineYield(CoroutineYield::new(progress.row().clone()));
                self.discover(yielded, pending);
                self.discover_type(progress.return_type(), pending);
            }
            // the handler environment, and the frames of the body and of each
            // clause
            AggregateType::RunWithFrames(instance) => {
                let layout = engine.build_run_with_layout(instance.clone()).await;
                for member in run_with_member_aggregates(&layout) {
                    self.discover(member, pending);
                }
            }
        }
    }

    /// Discovers the aggregates of the values an effect row resumes with and
    /// yields, which its resume and yield types share.
    async fn discover_row(
        &mut self,
        engine: &TrackedEngine,
        row: &EffectRow,
        pending: &mut Vec<AggregateType>,
    ) {
        let layout = engine.build_effect_row_layout(row.clone()).await;
        for (_, variant) in layout.resume_variants() {
            self.discover_type(variant.ty(), pending);
        }
        for (_, operation) in layout.row_operations() {
            self.discover_type(operation.arguments(), pending);
        }
    }

    /// Discovers every aggregate in `ty` not found before.
    fn discover_type(&mut self, ty: &MonoType, pending: &mut Vec<AggregateType>) {
        match ty {
            MonoType::Bool
            | MonoType::Int8
            | MonoType::Int16
            | MonoType::Int32
            | MonoType::Int64
            | MonoType::Isize
            | MonoType::Uint8
            | MonoType::Uint16
            | MonoType::Uint32
            | MonoType::Uint64
            | MonoType::Usize
            | MonoType::Float32
            | MonoType::CInt
            | MonoType::CStr
            | MonoType::OpaquePointer(_) => {}
            MonoType::Pointer(pointer) => self.discover_type(pointer.pointee(), pending),
            // check before cloning: most visits find an already-known type
            MonoType::Aggregate(aggregate) => {
                if !self.found.contains(aggregate) {
                    self.discover(aggregate.clone(), pending);
                }
            }
            MonoType::FunctionPointer(signature) => self.discover_signature(signature, pending),
        }
    }

    /// Discovers every aggregate in the slots of `signature`.
    fn discover_signature(
        &mut self,
        signature: &FunctionSignature,
        pending: &mut Vec<AggregateType>,
    ) {
        if let Some(slot) = signature.environment_slot() {
            self.discover_type(&slot.ty(), pending);
        }
        for parameter in signature.parameter_types() {
            self.discover_type(parameter, pending);
        }
        for effect in signature.handler_record_slots() {
            self.discover_handler_record(effect, pending);
        }
        match signature.return_type() {
            ReturnType::Void => {}
            ReturnType::Value(ty) => self.discover_type(ty, pending),
        }
    }

    /// Discovers the handler record of `effect`, which a pointer refers to.
    fn discover_handler_record(
        &mut self,
        effect: &MonoEffectInstance,
        pending: &mut Vec<AggregateType>,
    ) {
        self.discover(
            AggregateType::EffectHandlerCallback(EffectHandlerCallback::new(effect.clone())),
            pending,
        );
    }

    /// Marks `aggregate` found, and pending if it was not found before.
    fn discover(&mut self, aggregate: AggregateType, pending: &mut Vec<AggregateType>) {
        if !self.found.contains(&aggregate) {
            self.found.insert(aggregate.clone());
            pending.push(aggregate);
        }
    }
}

/// The aggregates that the frames of a `run ... with` hold: the environment of
/// its clauses, and the frames of its body and of each clause.
fn run_with_member_aggregates(layout: &RunWithLayout) -> impl Iterator<Item = AggregateType> + '_ {
    let environment = layout.handler_environment().cloned().map(AggregateType::Environment);
    let frames = std::iter::once(layout.body())
        .chain(layout.clauses().map(RunWithClause::coroutine))
        .cloned()
        .map(AggregateType::CoroutineFrame);

    environment.into_iter().chain(frames)
}
