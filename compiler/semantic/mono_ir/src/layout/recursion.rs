//! The frame graph, which tells where a coroutine frame holds other frames by
//! value, and so where a recursive coroutine holds its own frame.
//!
//! The graph is read from the frame field types before any boxing, which
//! [`BuildInlineFrameFieldTypes`] gives.
//!
//! [`BuildInlineFrameFieldTypes`]: crate::layout::coroutine::BuildInlineFrameFieldTypes

use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Identifiable, Query, StableHash, executor, program::Registration,
    storage::intern::Interned,
};
use rayc_coroutine_frame_layout::savable::Savable;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};

use crate::{
    MonoCoroutineInstance, MonoRunWithInstance,
    layout::{
        coroutine::build_inline_frame_field_types as _,
        run_with::{RunWithClause, build_run_with_layout as _},
    },
    ty::{AggregateType, MonoType},
};

/// A slot of a coroutine frame that holds other frames by value: an edge of
/// the frame graph, from the frame that has the slot to every frame in it.
///
/// Three kinds of slot hold a frame:
///
/// - the child frame of a call, which holds the callee's frame;
/// - the child frame of a `run ... with`, which holds the frame of the handled
///   body and that of every clause;
/// - a local or an expression whose type is a frame: the storage that a `coro`
///   builds its coroutine into, and the `Coroutine` expression that it stores
///   there.
///
/// No other value holds a frame by value. A pointer to a frame, such as the
/// operand of a continuation, is not an edge.
///
/// Boxing a slot removes all of its targets at once, so the slot, not a pair
/// of frames, is what the analysis chooses to box.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct FrameSlot {
    savable: Savable,
    targets: Interned<[MonoCoroutineInstance]>,
}

impl FrameSlot {
    /// The value of the owner's frame that holds the frames.
    #[must_use]
    pub const fn savable(&self) -> Savable { self.savable }

    /// The coroutines whose frames the slot holds, in `Ord` order, without
    /// repetition.
    #[must_use]
    pub fn targets(&self) -> impl ExactSizeIterator<Item = &MonoCoroutineInstance> {
        self.targets.iter()
    }
}

/// Finds the slots of the frame of a coroutine that hold other frames by
/// value: the successors of the coroutine in the frame graph.
#[derive(Debug, Clone, PartialEq, Eq, Hash, StableHash, Encode, Decode, Query)]
#[value(Interned<[FrameSlot]>)]
#[extend(name = get_frame_successors, by_val)]
pub struct FrameSuccessors {
    /// The coroutine whose frame has the slots.
    pub instance: MonoCoroutineInstance,
}

#[executor(config = Config)]
async fn frame_successors_executor(
    key: &FrameSuccessors,
    engine: &TrackedEngine,
) -> Interned<[FrameSlot]> {
    let types = engine.build_inline_frame_field_types(key.instance.clone()).await;

    // the fields are in `Savable` order, so the slots are too
    let mut slots = Vec::new();
    for (savable, ty) in types.fields() {
        let Some(held) = ty.held_frame() else {
            continue;
        };

        let mut targets = held.coroutines(engine).await;
        targets.sort_unstable();
        targets.dedup();

        slots.push(FrameSlot { savable, targets: engine.intern_unsized(targets) });
    }

    engine.intern_unsized(slots)
}

#[distributed_slice(RAY_PROGRAM)]
static FRAME_SUCCESSORS_EXECUTOR: Registration<Config> =
    Registration::new::<FrameSuccessors, FrameSuccessorsExecutor>();

/// The frames that a value holds by value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeldFrame<'a> {
    /// The frame of one coroutine.
    Coroutine(&'a MonoCoroutineInstance),
    /// The frames of the handled body and of every clause of a `run ...
    /// with`.
    RunWith(&'a MonoRunWithInstance),
}

impl HeldFrame<'_> {
    /// The coroutines whose frames are held.
    async fn coroutines(self, engine: &TrackedEngine) -> Vec<MonoCoroutineInstance> {
        match self {
            Self::Coroutine(coroutine) => vec![coroutine.clone()],
            Self::RunWith(run_with) => {
                let layout = engine.build_run_with_layout(run_with.clone()).await;
                std::iter::once(layout.body())
                    .chain(layout.clauses().map(RunWithClause::coroutine))
                    .cloned()
                    .collect()
            }
        }
    }
}

impl MonoType {
    /// The frames that a value of this type holds by value, if any: the edge
    /// of the frame graph that a slot of this type is.
    const fn held_frame(&self) -> Option<HeldFrame<'_>> {
        match self {
            Self::Aggregate(aggregate) => aggregate.held_frame(),

            // a pointer to a frame, such as the operand of a continuation,
            // only refers to a frame that something else holds
            Self::Bool
            | Self::Int8
            | Self::Int16
            | Self::Int32
            | Self::Int64
            | Self::Isize
            | Self::Uint8
            | Self::Uint16
            | Self::Uint32
            | Self::Uint64
            | Self::Usize
            | Self::Float32
            | Self::CInt
            | Self::CStr
            | Self::OpaquePointer(_)
            | Self::Pointer(_)
            | Self::FunctionPointer(_) => None,
        }
    }
}

impl AggregateType {
    /// The frames that a value of this aggregate holds by value, if any.
    const fn held_frame(&self) -> Option<HeldFrame<'_>> {
        match self {
            Self::CoroutineFrame(coroutine) => Some(HeldFrame::Coroutine(coroutine)),
            Self::RunWithFrames(run_with) => Some(HeldFrame::RunWith(run_with)),

            // No source value can hold a coroutine by value, so neither can a
            // tuple, a struct or an environment. The aggregates of a resume,
            // a yield and a progress carry values across a resume, which a
            // frame never is.
            Self::EffectHandlerCallback(_)
            | Self::Tuple(_)
            | Self::Environment(_)
            | Self::Struct(_)
            | Self::CoroutineResume(_)
            | Self::CoroutineYield(_)
            | Self::CoroutineProgress(_) => None,
        }
    }
}
