//! How a value of each type is represented at runtime.
//!
//! The representation is a property of the type alone, so every backend
//! agrees on it: a zero-sized value is never stored, and a transparent one is
//! stored as the only member it holds, whichever backend spells it.

use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Query, StableHash, executor, program::Registration, storage::intern::Interned,
};
use rayc_extend::extend;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};

use crate::{
    layout::{
        build_environment_layout, build_handler_layout, build_struct_layout,
        coroutine::build_effect_row_layout,
    },
    ty::{AggregateType, MonoType},
};

/// How a value of a type is represented at runtime.
#[derive(Debug, Clone, PartialEq, Eq, Hash, StableHash, Encode, Decode)]
pub enum Repr {
    /// The type has exactly one value, so a value of it holds nothing: it is
    /// never stored, passed or returned.
    ///
    /// A place of the type still has an address, which only tells it apart
    /// from no place at all.
    ZeroSized,

    /// The aggregate stores exactly one member, so a value of it is that
    /// member: it is stored, passed and returned as a value of the given
    /// type, and selecting the member selects the whole value.
    ///
    /// The type is the member's own representation, so it is never zero-sized
    /// or transparent itself: a wrapper of a wrapper is the innermost value.
    Transparent(Interned<MonoType>),

    /// The type is represented by itself, as a scalar or as an aggregate of
    /// its members whose types are not zero-sized.
    Direct,
}

impl Repr {
    /// Whether a value of the type holds nothing.
    #[must_use]
    pub const fn is_zero_sized(&self) -> bool {
        match self {
            Self::ZeroSized => true,
            Self::Transparent(_) | Self::Direct => false,
        }
    }

    /// The type a value of a transparent type is stored as, or `None` if the
    /// type is not transparent.
    #[must_use]
    pub const fn transparent_inner(&self) -> Option<&Interned<MonoType>> {
        match self {
            Self::Transparent(inner) => Some(inner),
            Self::ZeroSized | Self::Direct => None,
        }
    }

    /// Whether a value of the type is stored as itself, so an aggregate of
    /// the type is defined with its members.
    #[must_use]
    pub const fn is_direct(&self) -> bool {
        match self {
            Self::Direct => true,
            Self::ZeroSized | Self::Transparent(_) => false,
        }
    }
}

/// Derives the representation of an aggregate type.
///
/// A tuple, struct, environment or `coroutine_resume_t` is represented by
/// the members it stores, those whose types are not zero-sized: it is
/// zero-sized without any, transparent with exactly one, and direct with
/// more. An effect handler record is zero-sized without operations and
/// direct otherwise. An aggregate with a tag is always direct, since the tag
/// is stored beside its other members.
#[derive(Debug, Clone, PartialEq, Eq, Hash, StableHash, Encode, Decode, Query)]
#[value(Repr)]
#[extend(name = get_aggregate_repr, by_val)]
pub struct AggregateReprKey {
    /// The aggregate whose members determine its representation.
    pub aggregate: AggregateType,
}

#[executor(config = Config)]
async fn aggregate_repr_executor(key: &AggregateReprKey, engine: &TrackedEngine) -> Repr {
    match &key.aggregate {
        AggregateType::Tuple(tuple) => {
            repr_of_members(engine, tuple.fields().iter().cloned()).await
        }
        AggregateType::Struct(instance) => {
            let layout = engine.build_struct_layout(instance.clone()).await;
            repr_of_members(engine, layout.fields().values().cloned()).await
        }

        // a handler record is a pointer, which is stored like a capture
        AggregateType::Environment(instance) => {
            let layout = engine.build_environment_layout(instance.clone()).await;
            let records = layout
                .handler_records()
                .map(|(_, record)| MonoType::new_handler_pointer(record.effect().clone(), engine));
            let captures = layout.captures().map(|(_, capture)| capture.clone());
            repr_of_members(engine, captures.chain(records)).await
        }

        // each operation is a callback and an opaque environment pointer, so
        // a record with an operation stores at least two members
        AggregateType::EffectHandlerCallback(handler) => {
            let layout = engine.build_handler_layout(handler.mono_effect_instance().clone()).await;
            if layout.operations().is_empty() { Repr::ZeroSized } else { Repr::Direct }
        }

        // the members of a `coroutine_resume_t` overlap, all at its start, so
        // with only one stored it is that member
        AggregateType::CoroutineResume(resume) => {
            let layout = engine.build_effect_row_layout(resume.row().clone()).await;
            let variants = layout.resume_variants().map(|(_, variant)| variant.ty().clone());
            repr_of_members(engine, variants).await
        }

        // each of these stores a tag
        AggregateType::CoroutineFrame(_)
        | AggregateType::CoroutineYield(_)
        | AggregateType::CoroutineProgress(_)
        | AggregateType::RunWithFrames(_) => Repr::Direct,
    }
}

#[distributed_slice(RAY_PROGRAM)]
static AGGREGATE_REPR_EXECUTOR: Registration<Config> =
    Registration::new::<AggregateReprKey, AggregateReprExecutor>();

/// The representation of `ty`. Only an aggregate can be zero-sized or
/// transparent.
///
/// A pointer is direct whatever it points to, so the representation of a
/// type never asks for that of a type it only points to, and a type that
/// points to itself is no cycle.
#[extend]
pub async fn get_repr(self: &TrackedEngine, ty: &MonoType) -> Repr {
    match ty.as_aggregate() {
        Some(aggregate) => self.get_aggregate_repr(aggregate.clone()).await,
        None => Repr::Direct,
    }
}

/// The representation of an aggregate whose members are `members`, which it
/// stores side by side or overlapping, all from its start: zero-sized when
/// none holds anything, which includes having no member at all, transparent
/// when exactly one does, and direct otherwise.
async fn repr_of_members(
    engine: &TrackedEngine,
    members: impl IntoIterator<Item = Interned<MonoType>>,
) -> Repr {
    let mut stored = None;
    for member in members {
        let repr = engine.get_repr(&member).await;
        if repr.is_zero_sized() {
            continue;
        }
        if stored.is_some() {
            return Repr::Direct;
        }

        // a transparent member is stored as its own only member, so the
        // aggregate is too
        stored = Some(repr.transparent_inner().cloned().unwrap_or(member));
    }

    stored.map_or(Repr::ZeroSized, Repr::Transparent)
}
