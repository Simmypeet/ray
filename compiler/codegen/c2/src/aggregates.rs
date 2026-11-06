//! Discovery and ordering of the aggregate types used by generated code.

use qbice::storage::intern::Interned;
use rayc_hash::{FxHashMap, FxHashSet};
use rayc_mono_ir::{
    MonoCoroutineInstance, MonoEffectInstance, MonoStructInstance,
    layout::{
        HandlerLayout, StructLayout, build_handler_layout, build_struct_layout,
        coroutine::{
            EffectRowLayout, build_effect_row_layout, build_frame_field_types, build_frame_layout,
        },
    },
    ty::{AggregateType, CoroutineYield, EffectRow, FunctionSignature, MonoType, ReturnType},
};
use rayc_qbice::TrackedEngine;

use crate::c::{
    aggregate::{FrameMembers, NominalLayout},
    name::AggregateName,
};

/// Every aggregate type reachable from generated code, closed under the types
/// of their members.
///
/// Each insertion completes before returning: newly found aggregates have
/// their members, and nominal aggregates their layouts, registered as part of
/// the same call.
#[derive(Debug, Default)]
pub(crate) struct AggregateRegistry {
    /// Each registered aggregate with its precomputed C name.
    names: FxHashMap<AggregateType, AggregateName>,
    handler_layouts: FxHashMap<MonoEffectInstance, Interned<HandlerLayout>>,
    struct_layouts: FxHashMap<MonoStructInstance, Interned<StructLayout>>,
    frame_members: FxHashMap<MonoCoroutineInstance, FrameMembers>,
    row_layouts: FxHashMap<EffectRow, Interned<EffectRowLayout>>,
}

impl AggregateRegistry {
    /// Registers every aggregate that `ty` mentions.
    pub(crate) async fn insert_type(&mut self, engine: &TrackedEngine, ty: &MonoType) {
        let mut found = Vec::new();
        self.discover_type(ty, &mut found);
        self.complete(engine, found).await;
    }

    /// Registers every aggregate that `signature` mentions.
    pub(crate) async fn insert_signature(
        &mut self,
        engine: &TrackedEngine,
        signature: &FunctionSignature,
    ) {
        let mut found = Vec::new();
        self.discover_signature(signature, &mut found);
        self.complete(engine, found).await;
    }

    /// Registers `aggregate` itself.
    pub(crate) async fn insert(&mut self, engine: &TrackedEngine, aggregate: AggregateType) {
        let mut found = Vec::new();
        self.discover(aggregate, &mut found);
        self.complete(engine, found).await;
    }

    /// Registers the members of each newly `found` aggregate, which may find
    /// further aggregates in turn.
    ///
    /// `found` is a local worklist rather than async recursion, which would
    /// need a heap-allocated future per level.
    async fn complete(&mut self, engine: &TrackedEngine, mut found: Vec<AggregateType>) {
        while let Some(aggregate) = found.pop() {
            match &aggregate {
                AggregateType::EffectHandler(handler) => {
                    let instance = handler.mono_effect_instance();
                    let layout = engine.build_handler_layout(instance.clone()).await;
                    for operation in layout.operations() {
                        self.discover_signature(operation.signature(), &mut found);
                    }
                    self.handler_layouts.insert(instance.clone(), layout);
                }
                AggregateType::Tuple(tuple) => {
                    tuple.fields().iter().for_each(|field| self.discover_type(field, &mut found));
                }
                AggregateType::Environment(environment) => {
                    for capture in environment.captures() {
                        self.discover_type(capture, &mut found);
                    }
                }
                AggregateType::Struct(instance) => {
                    let layout = engine.build_struct_layout(instance.clone()).await;
                    let fields = layout.fields().values();
                    fields.for_each(|field| self.discover_type(field, &mut found));
                    self.struct_layouts.insert(instance.clone(), layout);
                }
                AggregateType::CoroutineFrame(instance) => {
                    let members = FrameMembers::new(
                        engine.build_frame_layout(instance.clone()).await,
                        engine.build_frame_field_types(instance.clone()).await,
                    );
                    members.types().for_each(|field| self.discover_type(field, &mut found));
                    self.frame_members.insert(instance.clone(), members);
                }
                AggregateType::CoroutineResume(resume) => {
                    self.complete_row(engine, resume.row(), &mut found).await;
                }
                AggregateType::CoroutineYield(yielded) => {
                    self.complete_row(engine, yielded.row(), &mut found).await;
                }
                AggregateType::CoroutineProgress(progress) => {
                    let yielded =
                        AggregateType::CoroutineYield(CoroutineYield::new(progress.row().clone()));
                    self.discover(yielded, &mut found);
                    self.discover_type(progress.return_type(), &mut found);
                }
            }
        }
    }

    /// Registers the layout of `row` and every type its members mention.
    async fn complete_row(
        &mut self,
        engine: &TrackedEngine,
        row: &EffectRow,
        found: &mut Vec<AggregateType>,
    ) {
        if self.row_layouts.contains_key(row) {
            return;
        }

        let layout = engine.build_effect_row_layout(row.clone()).await;
        for (_, variant) in layout.resume_variants() {
            self.discover_type(variant.ty(), found);
        }
        for (_, operation) in layout.row_operations() {
            self.discover_type(operation.arguments(), found);
        }
        self.row_layouts.insert(row.clone(), layout);
    }

    /// Names every aggregate in `ty` not seen before, adding it to `found`.
    fn discover_type(&mut self, ty: &MonoType, found: &mut Vec<AggregateType>) {
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
            MonoType::Pointer(pointer) => self.discover_type(pointer.pointee(), found),
            // Check before cloning: most visits find an already-known type.
            MonoType::Aggregate(aggregate) => {
                if !self.names.contains_key(aggregate) {
                    self.discover(aggregate.clone(), found);
                }
            }
            MonoType::FunctionPointer(signature) => self.discover_signature(signature, found),
        }
    }

    fn discover_signature(
        &mut self,
        signature: &FunctionSignature,
        found: &mut Vec<AggregateType>,
    ) {
        for parameter in signature.parameter_types() {
            self.discover_type(parameter, found);
        }
        match signature.return_type() {
            ReturnType::Void => {}
            ReturnType::Value(ty) => self.discover_type(ty, found),
        }
    }

    fn discover(&mut self, aggregate: AggregateType, found: &mut Vec<AggregateType>) {
        if !self.names.contains_key(&aggregate) {
            self.names.insert(aggregate.clone(), AggregateName::of(&aggregate));
            found.push(aggregate);
        }
    }

    /// The layout of `aggregate`, if a query tells it.
    fn layout_of(&self, aggregate: &AggregateType) -> NominalLayout<'_> {
        match aggregate {
            AggregateType::EffectHandler(handler) => NominalLayout::EffectHandler(
                self.handler_layouts
                    .get(handler.mono_effect_instance())
                    .expect("a registered effect handler should have its layout"),
            ),
            AggregateType::CoroutineFrame(instance) => NominalLayout::CoroutineFrame(
                self.frame_members
                    .get(instance)
                    .expect("a registered coroutine frame should have its members"),
            ),
            AggregateType::CoroutineResume(resume) => {
                NominalLayout::EffectRow(self.row_layout(resume.row()))
            }
            AggregateType::CoroutineYield(yielded) => {
                NominalLayout::EffectRow(self.row_layout(yielded.row()))
            }
            AggregateType::Struct(instance) => NominalLayout::Struct(self.struct_layout(instance)),
            AggregateType::Tuple(_)
            | AggregateType::Environment(_)
            | AggregateType::CoroutineProgress(_) => NominalLayout::None,
        }
    }

    fn struct_layout(&self, instance: &MonoStructInstance) -> &StructLayout {
        self.struct_layouts.get(instance).expect("a registered struct should have its layout")
    }

    fn row_layout(&self, row: &EffectRow) -> &EffectRowLayout {
        self.row_layouts.get(row).expect("a registered effect row should have its layout")
    }

    /// Every aggregate with its nominal layout, ordered so that each one
    /// follows the aggregates it contains by value, as C requires complete
    /// member types. Ties are broken by name for determinism.
    pub(crate) fn in_definition_order(
        &self,
    ) -> impl Iterator<Item = (AggregateName, &AggregateType, NominalLayout<'_>)> {
        let mut roots =
            self.names.iter().map(|(aggregate, name)| (*name, aggregate)).collect::<Vec<_>>();
        roots.sort_unstable_by_key(|(name, _)| *name);

        let mut ordering = TopologicalOrdering::default();
        for (name, aggregate) in roots {
            ordering.visit(self, name, aggregate);
        }
        ordering
            .ordered
            .into_iter()
            .map(|(name, aggregate)| (name, aggregate, self.layout_of(aggregate)))
    }

    /// The aggregates that `aggregate` stores by value, sorted by name.
    fn by_value_dependencies<'a>(
        &'a self,
        aggregate: &'a AggregateType,
    ) -> Vec<(AggregateName, &'a AggregateType)> {
        match aggregate {
            // Handler members are pointers, which need no complete type.
            AggregateType::EffectHandler(_) => Vec::new(),
            AggregateType::Tuple(tuple) => self.named_aggregates(tuple.fields()),
            AggregateType::Environment(environment) => {
                self.named_aggregates(environment.captures())
            }
            AggregateType::Struct(instance) => {
                self.named_aggregates(self.struct_layout(instance).fields().values())
            }
            AggregateType::CoroutineFrame(instance) => {
                let members = self
                    .frame_members
                    .get(instance)
                    .expect("a registered coroutine frame should have its members");
                self.named_aggregates(members.types())
            }
            AggregateType::CoroutineResume(resume) => self.named_aggregates(
                self.row_layout(resume.row()).resume_variants().map(|(_, variant)| variant.ty()),
            ),
            AggregateType::CoroutineYield(yielded) => self.named_aggregates(
                self.row_layout(yielded.row())
                    .row_operations()
                    .map(|(_, operation)| operation.arguments()),
            ),
            AggregateType::CoroutineProgress(progress) => {
                let yielded = self
                    .names
                    .get_key_value(&AggregateType::CoroutineYield(CoroutineYield::new(
                        progress.row().clone(),
                    )))
                    .expect("a registered progress type should have its yield type");
                let mut dependencies = self.named_aggregates([progress.return_type()]);
                dependencies.push((*yielded.1, yielded.0));
                dependencies.sort_unstable_by_key(|(name, _)| *name);
                dependencies
            }
        }
    }

    fn named_aggregates<'a>(
        &self,
        types: impl IntoIterator<Item = &'a Interned<MonoType>>,
    ) -> Vec<(AggregateName, &'a AggregateType)> {
        let mut dependencies = types
            .into_iter()
            .filter_map(|ty| by_value_aggregate(ty))
            .map(|aggregate| {
                let name = *self
                    .names
                    .get(aggregate)
                    .expect("aggregate dependency should have passed through the registry");
                (name, aggregate)
            })
            .collect::<Vec<_>>();
        dependencies.sort_unstable_by_key(|(name, _)| *name);
        dependencies
    }
}

/// The aggregate a value of type `ty` contains inline, if any.
const fn by_value_aggregate(ty: &MonoType) -> Option<&AggregateType> {
    match ty {
        MonoType::Aggregate(aggregate) => Some(aggregate),
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
        | MonoType::OpaquePointer(_)
        | MonoType::Pointer(_)
        | MonoType::FunctionPointer(_) => None,
    }
}

/// Depth-first post-order over by-value containment.
#[derive(Debug, Default)]
struct TopologicalOrdering<'a> {
    visiting: FxHashSet<&'a AggregateType>,
    visited: FxHashSet<&'a AggregateType>,
    ordered: Vec<(AggregateName, &'a AggregateType)>,
}

impl<'a> TopologicalOrdering<'a> {
    fn visit(
        &mut self,
        registry: &'a AggregateRegistry,
        name: AggregateName,
        aggregate: &'a AggregateType,
    ) {
        if self.visited.contains(aggregate) {
            return;
        }
        assert!(self.visiting.insert(aggregate), "aggregate types cannot contain a cycle by value");

        for (dependency_name, dependency) in registry.by_value_dependencies(aggregate) {
            self.visit(registry, dependency_name, dependency);
        }

        self.visiting.remove(aggregate);
        self.visited.insert(aggregate);
        self.ordered.push((name, aggregate));
    }
}
