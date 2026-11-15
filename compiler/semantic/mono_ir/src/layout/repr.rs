//! How a value of each type is represented at runtime.
//!
//! The representation is a property of the type alone, so every backend
//! agrees on it: a zero-sized value is never stored, whichever backend spells
//! it.

use linkme::distributed_slice;
use qbice::{Decode, Encode, Query, StableHash, executor, program::Registration};
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, StableHash, Encode, Decode)]
pub enum Repr {
    /// The type has exactly one value, so a value of it holds nothing: it is
    /// never stored, passed or returned.
    ///
    /// A place of the type still has an address, which only tells it apart
    /// from no place at all.
    ZeroSized,

    /// The type is represented by itself, as a scalar or as an aggregate of
    /// its members whose types are not zero-sized.
    Direct,
}

impl Repr {
    /// Whether a value of the type holds nothing.
    #[must_use]
    pub const fn is_zero_sized(self) -> bool {
        match self {
            Self::ZeroSized => true,
            Self::Direct => false,
        }
    }
}

/// Derives the representation of an aggregate type.
///
/// An aggregate is zero-sized when every member it stores is: a tuple, struct,
/// environment or `coroutine_resume_t` whose members are all zero-sized, and
/// an effect handler record without operations. An aggregate with a tag is
/// never zero-sized, since the tag is stored.
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
            zero_sized_when_all_are(engine, tuple.fields().iter().map(|field| &**field)).await
        }
        AggregateType::Struct(instance) => {
            let layout = engine.build_struct_layout(instance.clone()).await;
            zero_sized_when_all_are(engine, layout.fields().values().map(|field| &**field)).await
        }

        // A handler record is a pointer, which is always stored.
        AggregateType::Environment(instance) => {
            let layout = engine.build_environment_layout(instance.clone()).await;
            if layout.handler_records().len() != 0 {
                return Repr::Direct;
            }
            zero_sized_when_all_are(engine, layout.captures().map(|(_, capture)| &**capture)).await
        }

        // Each operation is a callback and an opaque environment pointer.
        AggregateType::EffectHandlerCallback(handler) => {
            let layout = engine.build_handler_layout(handler.mono_effect_instance().clone()).await;
            if layout.operations().is_empty() { Repr::ZeroSized } else { Repr::Direct }
        }

        // The members of a `coroutine_resume_t` overlap, so it stores the
        // largest of them.
        AggregateType::CoroutineResume(resume) => {
            let layout = engine.build_effect_row_layout(resume.row().clone()).await;
            let variants = layout.resume_variants().map(|(_, variant)| &**variant.ty());
            zero_sized_when_all_are(engine, variants).await
        }

        // Each of these stores a tag.
        AggregateType::CoroutineFrame(_)
        | AggregateType::CoroutineYield(_)
        | AggregateType::CoroutineProgress(_)
        | AggregateType::RunWithFrames(_) => Repr::Direct,
    }
}

#[distributed_slice(RAY_PROGRAM)]
static AGGREGATE_REPR_EXECUTOR: Registration<Config> =
    Registration::new::<AggregateReprKey, AggregateReprExecutor>();

/// The representation of `ty`. Only an aggregate can be zero-sized.
#[extend]
pub async fn get_repr(self: &TrackedEngine, ty: &MonoType) -> Repr {
    match ty.as_aggregate() {
        Some(aggregate) => self.get_aggregate_repr(aggregate.clone()).await,
        None => Repr::Direct,
    }
}

/// The representation of an aggregate that stores `members`: zero-sized when
/// every member is, which includes an aggregate without members.
async fn zero_sized_when_all_are<'a>(
    engine: &TrackedEngine,
    members: impl IntoIterator<Item = &'a MonoType>,
) -> Repr {
    for member in members {
        if !engine.get_repr(member).await.is_zero_sized() {
            return Repr::Direct;
        }
    }
    Repr::ZeroSized
}
