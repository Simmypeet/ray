//! The aggregate types of the whole program, found once every function is
//! generated.
//!
//! Each function reports only the aggregates its code spells. Defining an
//! aggregate spells the types of its members, so those must be declared too,
//! and the ones held by value defined before it. They are found here, once
//! for the whole program: the members of each aggregate are fetched from its
//! layout query once, and the same members then order the definitions and
//! print them.

use std::collections::BTreeMap;

use rayc_coroutine_frame_layout::frame_layout::Placement;
use rayc_hash::FxHashSet;
use rayc_mono_ir::{
    MonoCoroutineInstance, MonoEffectInstance, MonoEnumInstance, MonoEnvironmentInstance,
    MonoRunWithInstance, MonoStructInstance,
    layout::{
        build_enum_layout, build_environment_layout, build_handler_layout, build_struct_layout,
        coroutine::{
            FrameState, build_effect_row_layout, build_frame_field_types, build_frame_layout,
        },
        repr::{TypeRepr, get_aggregate_repr},
        run_with::build_run_with_layout,
    },
    ty::{
        AggregateType, CoroutineProgress, CoroutineYield, EffectHandlerCallback, EffectRow,
        FunctionSignature, MonoType, ReturnType, Tuple,
    },
};
use rayc_qbice::TrackedEngine;

use crate::c::{
    aggregate::{AggregateDefinition, Member, Members},
    name::{AggregateName, FieldName},
};

/// Every aggregate type the program needs: the ones its functions spell, and
/// every one their members mention in turn.
#[derive(Debug)]
pub(crate) struct ProgramAggregates {
    /// Every aggregate by name, so iterating them does not depend on the
    /// order the functions were generated in.
    aggregates: BTreeMap<AggregateName, FoundAggregate>,
}

/// An aggregate the program needs, with what defining it takes.
#[derive(Debug)]
struct FoundAggregate {
    aggregate: AggregateType,
    /// The members that hold something: a member of a zero-sized type is
    /// left out, and so is a union or struct of nothing else.
    members: Members,
    /// How a value of it is represented. Only a direct aggregate is defined;
    /// any other is only declared, and spelled as a struct only behind a
    /// pointer.
    repr: TypeRepr,
}

impl ProgramAggregates {
    /// The aggregates in `spelled`, which may repeat, and every aggregate
    /// their members mention.
    pub(crate) async fn new<'a>(
        engine: &TrackedEngine,
        spelled: impl IntoIterator<Item = &'a AggregateType>,
    ) -> Self {
        // the sets of the functions overlap, so they are merged first
        let mut discovery = Discovery::default();
        for aggregate in spelled {
            discovery.discover(aggregate);
        }

        // the members of each aggregate are fetched once, and may mention
        // aggregates not found before, which are fetched in turn; a member
        // that holds nothing is still declared, so it is found too
        let mut aggregates = BTreeMap::new();
        while let Some(aggregate) = discovery.pending.pop() {
            let mut members = members_of(engine, &aggregate).await;
            let mut member_aggregates = Vec::new();
            members.for_each_type(&mut |ty| {
                discovery.discover_type(ty);
                if let Some(member) = ty.as_aggregate() {
                    member_aggregates.push(member.clone());
                }
            });

            // only the members that hold something are defined
            let mut zero_sized = FxHashSet::default();
            for member in member_aggregates {
                if engine.get_aggregate_repr(member.clone()).await.is_zero_sized() {
                    zero_sized.insert(member);
                }
            }
            members.retain_stored(&|ty| ty.as_aggregate().is_none_or(|a| !zero_sized.contains(a)));

            let repr = engine.get_aggregate_repr(aggregate.clone()).await;
            let name = AggregateName::of(&aggregate);
            aggregates.insert(name, FoundAggregate { aggregate, members, repr });
        }

        Self { aggregates }
    }

    /// Every aggregate with its name, sorted by name.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (AggregateName, &AggregateType)> {
        self.aggregates.iter().map(|(name, found)| (*name, &found.aggregate))
    }

    /// Whether `aggregate` is among the aggregates.
    pub(crate) fn contains(&self, aggregate: &AggregateType) -> bool {
        self.aggregates.contains_key(&AggregateName::of(aggregate))
    }

    /// The definition of every aggregate that is defined, each after the
    /// aggregates it holds by value, since C needs the complete type of a
    /// member. Aggregates that do not depend on each other are ordered by
    /// name.
    ///
    /// Only a direct aggregate is defined. Any other is only declared: a
    /// value of a zero-sized one is never stored, and one of a transparent
    /// one is stored as its member, so either is spelled as its struct only
    /// behind a pointer.
    pub(crate) fn definitions(&self) -> impl Iterator<Item = AggregateDefinition<'_>> {
        let mut ordering = TopologicalOrdering::default();
        for name in self.aggregates.keys() {
            ordering.visit(self, *name);
        }

        ordering.ordered.into_iter().filter_map(|name| {
            let found = &self.aggregates[&name];
            found.repr.is_direct().then(|| AggregateDefinition::new(name, &found.members))
        })
    }

    /// The names of the aggregates that the aggregate called `name` holds by
    /// value, sorted. A pointer needs no complete type, so the aggregates
    /// behind one are not among them.
    fn by_value_dependencies(&self, name: AggregateName) -> Vec<AggregateName> {
        let mut dependencies = Vec::new();
        self.aggregates[&name].members.for_each_type(&mut |ty| {
            if let Some(stored) = self.stored_aggregate(ty) {
                dependencies.push(AggregateName::of(stored));
            }
        });
        dependencies.sort_unstable();
        dependencies
    }

    /// The aggregate whose definition a value of `ty` is spelled with, if
    /// any: a transparent aggregate is spelled as the member it stores, and a
    /// member is the innermost such, so it is direct if it is an aggregate.
    fn stored_aggregate<'a>(&'a self, ty: &'a MonoType) -> Option<&'a AggregateType> {
        let aggregate = ty.as_aggregate()?;
        let repr = &self.aggregates[&AggregateName::of(aggregate)].repr;
        repr.transparent_inner().map_or(Some(aggregate), |inner| inner.as_aggregate())
    }
}

/// Every member of `aggregate`, as the layout query of its kind tells,
/// including the ones that hold nothing.
async fn members_of(engine: &TrackedEngine, aggregate: &AggregateType) -> Members {
    match aggregate {
        AggregateType::Tuple(tuple) => tuple_members(tuple),
        AggregateType::Struct(instance) => struct_members(engine, instance).await,
        AggregateType::Enum(instance) => enum_members(engine, instance).await,
        AggregateType::Environment(instance) => environment_members(engine, instance).await,
        AggregateType::EffectHandlerCallback(handler) => {
            handler_members(engine, handler.mono_effect_instance()).await
        }
        AggregateType::CoroutineFrame(instance) => frame_members(engine, instance).await,
        AggregateType::CoroutineResume(resume) => resume_members(engine, resume.row()).await,
        AggregateType::CoroutineYield(yielded) => yield_members(engine, yielded.row()).await,
        AggregateType::CoroutineProgress(progress) => progress_members(engine, progress),
        AggregateType::RunWithFrames(instance) => run_with_members(engine, instance).await,
    }
}

/// `{ elem0; elem1; ... }`.
fn tuple_members(tuple: &Tuple) -> Members {
    tuple
        .fields()
        .iter()
        .enumerate()
        .map(|(index, ty)| {
            let index = u32::try_from(index).expect("a tuple index should fit in u32");
            Member::field(FieldName::Tuple(index), ty.clone())
        })
        .collect()
}

/// A member per field, in the order of the fields' IDs.
async fn struct_members(engine: &TrackedEngine, instance: &MonoStructInstance) -> Members {
    let layout = engine.build_struct_layout(instance.clone()).await;
    layout
        .fields()
        .iter()
        .map(|(field, ty)| Member::field(FieldName::Struct(*field), ty.clone()))
        .collect()
}

/// `{ tag; union { payload of each variant } }`: the tag tells which variant
/// the enum holds, and so which payload is defined. An enum of one variant, or
/// none, has no tag, so it is only the payload, if any.
async fn enum_members(engine: &TrackedEngine, instance: &MonoEnumInstance) -> Members {
    let layout = engine.build_enum_layout(instance.clone()).await;
    let payloads = layout.variants().filter_map(|(index, variant)| {
        let payload = variant.payload()?.clone();
        Some(Member::field(FieldName::VariantPayload(index), payload))
    });
    if !layout.has_tag() {
        return payloads.collect();
    }

    let tag = engine.intern(layout.discriminant_type().mono_type());
    [Member::field(FieldName::Tag, tag), Member::Union(payloads.collect())].into_iter().collect()
}

/// A member per capture, then a `const` pointer per handler record: a
/// closure that performs an effect calls the handler it was created under.
async fn environment_members(
    engine: &TrackedEngine,
    instance: &MonoEnvironmentInstance,
) -> Members {
    let layout = engine.build_environment_layout(instance.clone()).await;
    let captures = layout
        .captures()
        .map(|(capture, ty)| Member::field(FieldName::EnvironmentCapture(capture), ty.clone()));
    let records = layout.handler_records().map(|(record, handler)| {
        let ty = MonoType::new_handler_pointer(handler.effect().clone(), engine);
        Member::field(FieldName::EnvironmentHandlerRecord(record), ty)
    });
    captures.chain(records).collect()
}

/// Each operation of the effect is a closure: the environment it closes
/// over, and the function it calls with it, both pointers.
async fn handler_members(engine: &TrackedEngine, effect: &MonoEffectInstance) -> Members {
    let layout = engine.build_handler_layout(effect.clone()).await;
    let mut members = Members::default();
    for operation in layout.operations() {
        let signature = operation.signature();
        let environment = signature
            .environment_slot()
            .expect("an operation signature should have an environment slot")
            .ty();
        let id = operation.operation_id();
        members
            .push(Member::field(FieldName::OperationEnvironment(id), engine.intern(environment)));
        let function = engine.intern(MonoType::FunctionPointer(signature.clone()));
        members.push(Member::field(FieldName::OperationFunction(id), function));
    }
    members
}

/// `{ state; prefix; union { struct { variant }... } }`: the state tag, the
/// values every state keeps, and then the values each state keeps alone,
/// which overlap with the other states'.
///
/// The structs and the union are anonymous, so a value is accessed the same
/// way wherever the layout places it; see
/// [`FieldName::CoroutineFrameSavable`].
async fn frame_members(engine: &TrackedEngine, instance: &MonoCoroutineInstance) -> Members {
    let layout = engine.build_frame_layout(instance.clone()).await;
    let types = engine.build_frame_field_types(instance.clone()).await;
    let mut members = Members::default();

    let state = engine.intern(FrameState::tag_type(&layout).mono_type());
    members.push(Member::field(FieldName::CoroutineFrameState, state));

    // the prefix sorts first, then the variants by suspension point, so the
    // members do not depend on the hash order of the layout
    let mut placements =
        layout.placements().map(|(savable, placement)| (placement, savable)).collect::<Vec<_>>();
    placements.sort_unstable();
    let savable_member = |savable| {
        Member::field(FieldName::CoroutineFrameSavable(savable), types.get(savable).clone())
    };

    let prefix_len = placements.partition_point(|(placement, _)| *placement == Placement::Prefix);
    let (prefix, variants) = placements.split_at(prefix_len);
    for (_, savable) in prefix {
        members.push(savable_member(*savable));
    }
    let variants = variants
        .chunk_by(|left, right| left.0 == right.0)
        .map(|variant| {
            Member::Struct(variant.iter().map(|(_, savable)| savable_member(*savable)).collect())
        })
        .collect::<Members>();
    members.push(Member::Union(variants));
    members
}

/// `{ union { value of each resume variant } }`: a coroutine is resumed with
/// one of them.
async fn resume_members(engine: &TrackedEngine, row: &EffectRow) -> Members {
    let layout = engine.build_effect_row_layout(row.clone()).await;
    let variants = layout
        .resume_variants()
        .map(|(id, variant)| {
            Member::field(FieldName::CoroutineResumeValue(id), variant.ty().clone())
        })
        .collect();
    std::iter::once(Member::Union(variants)).collect()
}

/// `{ tag; union { arguments of each operation } }`: the tag tells which
/// operation the coroutine yielded with.
async fn yield_members(engine: &TrackedEngine, row: &EffectRow) -> Members {
    let layout = engine.build_effect_row_layout(row.clone()).await;
    let tag = engine.intern(layout.yield_tag_type().mono_type());
    let operations = layout
        .row_operations()
        .map(|(id, operation)| {
            Member::field(FieldName::CoroutineYieldOperation(id), operation.arguments().clone())
        })
        .collect();
    [Member::field(FieldName::Tag, tag), Member::Union(operations)].into_iter().collect()
}

/// `{ tag; union { yield_value; return_value } }`: a resumed coroutine either
/// yields or returns.
fn progress_members(engine: &TrackedEngine, progress: &CoroutineProgress) -> Members {
    let tag = engine.intern(CoroutineProgress::TAG_TYPE.mono_type());
    let yielded = MonoType::new_aggregate(
        AggregateType::CoroutineYield(CoroutineYield::new(progress.row().clone())),
        engine,
    );
    let outcomes = [
        Member::field(FieldName::CoroutineProgressYield, yielded),
        Member::field(FieldName::CoroutineProgressReturn, progress.return_type().clone()),
    ];
    [Member::field(FieldName::Tag, tag), Member::Union(outcomes.into_iter().collect())]
        .into_iter()
        .collect()
}

/// `{ handler_environment; body; phase; union { clause frames } }`: the frame
/// of the body, and the frame of whichever clause is running. A clause frame
/// is named by its operation alone.
async fn run_with_members(engine: &TrackedEngine, instance: &MonoRunWithInstance) -> Members {
    let layout = engine.build_run_with_layout(instance.clone()).await;
    let frame = |instance: &MonoCoroutineInstance| {
        MonoType::new_aggregate(AggregateType::CoroutineFrame(instance.clone()), engine)
    };
    let mut members = Members::default();

    // only a `run ... with` with clauses has an environment for them
    if let Some(environment) = layout.handler_environment() {
        let environment =
            MonoType::new_aggregate(AggregateType::Environment(environment.clone()), engine);
        members.push(Member::field(FieldName::RunWithHandlerEnvironment, environment));
    }
    members.push(Member::field(FieldName::RunWithBody, frame(layout.body())));
    let phase = engine.intern(layout.phase_tag_type().mono_type());
    members.push(Member::field(FieldName::RunWithPhase, phase));

    let clauses = layout
        .clauses()
        .map(|clause| {
            Member::field(
                FieldName::RunWithClause(clause.operation_id()),
                frame(clause.coroutine()),
            )
        })
        .collect();
    members.push(Member::Union(clauses));
    members
}

/// The aggregates found so far, and the ones whose members are still to be
/// looked at.
#[derive(Debug, Default)]
struct Discovery {
    found: FxHashSet<AggregateType>,
    pending: Vec<AggregateType>,
}

impl Discovery {
    /// Marks `aggregate` found, and pending if it was not found before.
    fn discover(&mut self, aggregate: &AggregateType) {
        // check before cloning: most aggregates are found more than once
        if !self.found.contains(aggregate) {
            self.found.insert(aggregate.clone());
            self.pending.push(aggregate.clone());
        }
    }

    /// Discovers every aggregate that spelling `ty` spells, even behind a
    /// pointer: a pointer to an aggregate still needs its `typedef`.
    fn discover_type(&mut self, ty: &MonoType) {
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
            MonoType::Pointer(pointer) => self.discover_type(pointer.pointee()),
            MonoType::Aggregate(aggregate) => self.discover(aggregate),
            MonoType::FunctionPointer(signature) => self.discover_signature(signature),
        }
    }

    /// Discovers every aggregate in the slots of `signature`, in the order C
    /// spells them.
    fn discover_signature(&mut self, signature: &FunctionSignature) {
        if let Some(slot) = signature.environment_slot() {
            self.discover_type(&slot.ty());
        }
        for parameter in signature.parameter_types() {
            self.discover_type(parameter);
        }
        for effect in signature.handler_record_slots() {
            self.discover_handler_record(effect);
        }
        match signature.return_type() {
            ReturnType::Void => {}
            ReturnType::Value(ty) => self.discover_type(ty),
        }
    }

    /// Discovers the handler record of `effect`, which a signature takes by
    /// pointer.
    fn discover_handler_record(&mut self, effect: &MonoEffectInstance) {
        let record =
            AggregateType::EffectHandlerCallback(EffectHandlerCallback::new(effect.clone()));
        self.discover(&record);
    }
}

/// A depth-first post-order walk over by-value containment.
#[derive(Debug, Default)]
struct TopologicalOrdering {
    /// The aggregates whose dependencies are being visited, which a cycle
    /// would visit again.
    visiting: FxHashSet<AggregateName>,
    visited: FxHashSet<AggregateName>,
    ordered: Vec<AggregateName>,
}

impl TopologicalOrdering {
    fn visit(&mut self, aggregates: &ProgramAggregates, name: AggregateName) {
        if self.visited.contains(&name) {
            return;
        }
        assert!(self.visiting.insert(name), "an aggregate cannot contain itself by value");

        for dependency in aggregates.by_value_dependencies(name) {
            self.visit(aggregates, dependency);
        }

        self.visiting.remove(&name);
        self.visited.insert(name);
        self.ordered.push(name);
    }
}
