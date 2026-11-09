//! The frame graph, which tells where a coroutine frame holds other frames by
//! value, and so where a recursive coroutine holds its own frame.
//!
//! Such a frame would have no finite size, so some of its slots are boxed: the
//! slot keeps a pointer, and the frames it held live on the heap. The slots to
//! box are chosen in four queries:
//!
//! 1. [`FrameSuccessors`] reads the slots of one frame that hold other frames:
//!    the edges of the graph.
//! 2. [`SccRepresentative`] finds the strongly connected component of a
//!    coroutine, and names its smallest member, unless the component has no
//!    cycle.
//! 3. [`BreakRecursion`] chooses, once per component, as few slots as it can
//!    whose boxing breaks every cycle in it.
//! 4. [`BoxingEdges`] gives one coroutine its part of that choice, which
//!    [`BuildFrameFieldTypes`] turns into pointer members.
//!
//! The graph is read from the frame field types before any boxing, which
//! [`BuildInlineFrameFieldTypes`] gives, so no query depends on its own
//! answer.
//!
//! [`BuildInlineFrameFieldTypes`]: crate::layout::coroutine::BuildInlineFrameFieldTypes
//! [`BuildFrameFieldTypes`]: crate::layout::coroutine::BuildFrameFieldTypes

mod solver;

use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Identifiable, Query, StableHash, executor, program::Registration,
    storage::intern::Interned,
};
use rayc_coroutine_frame_layout::savable::Savable;
use rayc_hash::FxHashMap;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_transitive_closure::BitSet;

use crate::{
    MonoCoroutineInstance, MonoRunWithInstance,
    layout::{
        coroutine::build_inline_frame_field_types as _,
        recursion::solver::Component,
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

/// Finds the representative of the strongly connected component of the frame
/// graph that a coroutine is in: its smallest member by `Ord`.
///
/// A component holds a cycle unless it is one member that holds itself in no
/// slot. Such a trivial component has no representative, and nothing in it is
/// boxed.
///
/// Every member finds the same representative, so the slots of the component
/// are chosen once, by [`BreakRecursion`], for all of them.
#[derive(Debug, Clone, PartialEq, Eq, Hash, StableHash, Encode, Decode, Query)]
#[value(Option<MonoCoroutineInstance>)]
#[extend(name = get_scc_representative, by_val)]
pub struct SccRepresentative {
    /// The coroutine whose component is requested.
    pub instance: MonoCoroutineInstance,
}

#[executor(config = Config)]
async fn scc_representative_executor(
    key: &SccRepresentative,
    engine: &TrackedEngine,
) -> Option<MonoCoroutineInstance> {
    let graph = ReachableGraph::explore(key.instance.clone(), engine).await;
    let members = graph.component_of_start();

    // a single member is on a cycle only if it holds itself
    let is_trivial = match members.as_slice() {
        [member] => !graph.holds_itself(member),
        _ => false,
    };

    if is_trivial { None } else { members.into_iter().next() }
}

#[distributed_slice(RAY_PROGRAM)]
static SCC_REPRESENTATIVE_EXECUTOR: Registration<Config> =
    Registration::new::<SccRepresentative, SccRepresentativeExecutor>();

/// Chooses the slots to box in the strongly connected component of the frame
/// graph whose representative is given, so that no frame holds itself.
///
/// A boxed slot keeps a pointer to the frames it holds, which live on the
/// heap. The choice maps each member that has a boxed slot to those slots, in
/// `Savable` order. A member without one is not in the map.
///
/// The choice breaks every cycle of the component, and boxes as few slots as
/// it can: exactly as few for a component of a few members, which is all that
/// real programs have.
///
/// Every slot weighs the same for now. A slot inside a loop allocates more
/// often, but the IR has no loop analysis yet.
#[derive(Debug, Clone, PartialEq, Eq, Hash, StableHash, Encode, Decode, Query)]
#[value(Interned<FxHashMap<MonoCoroutineInstance, Interned<[Savable]>>>)]
#[extend(name = break_recursion, by_val)]
pub struct BreakRecursion {
    /// The representative of the component, which [`SccRepresentative`]
    /// gives.
    pub representative: MonoCoroutineInstance,
}

#[executor(config = Config)]
async fn break_recursion_executor(
    key: &BreakRecursion,
    engine: &TrackedEngine,
) -> Interned<FxHashMap<MonoCoroutineInstance, Interned<[Savable]>>> {
    let graph = ReachableGraph::explore(key.representative.clone(), engine).await;
    let members = graph.component_of_start();
    assert_eq!(
        members.first(),
        Some(&key.representative),
        "the recursion of a component should be broken from its representative"
    );

    // the members are numbered in `Ord` order, and their slots in `Savable`
    // order, so the choice does not depend on how the component was found
    let numbers = members
        .iter()
        .enumerate()
        .map(|(number, member)| (member, number))
        .collect::<FxHashMap<_, _>>();
    let mut component = Component::new(members.len());
    let mut slots = Vec::new();
    for (owner, member) in members.iter().enumerate() {
        for slot in graph.slots_of(member) {
            // a slot whose targets are all outside the component is on no
            // cycle
            let targets = slot.targets().filter_map(|target| numbers.get(target).copied());
            let targets = targets.collect::<Vec<_>>();
            if targets.is_empty() {
                continue;
            }

            component.add_slot(owner, targets, 1);
            slots.push((owner, slot.savable()));
        }
    }

    // the chosen slots come in increasing order, so each owner's savables
    // stay in `Savable` order
    let mut boxed = FxHashMap::<MonoCoroutineInstance, Vec<Savable>>::default();
    for slot in component.break_cycles() {
        let (owner, savable) = slots[slot];
        boxed.entry(members[owner].clone()).or_default().push(savable);
    }

    engine.intern(
        boxed
            .into_iter()
            .map(|(owner, savables)| (owner, engine.intern_unsized(savables)))
            .collect(),
    )
}

#[distributed_slice(RAY_PROGRAM)]
static BREAK_RECURSION_EXECUTOR: Registration<Config> =
    Registration::new::<BreakRecursion, BreakRecursionExecutor>();

/// Finds the slots of the frame of a coroutine that are boxed, so that no
/// frame holds itself.
#[derive(Debug, Clone, PartialEq, Eq, Hash, StableHash, Encode, Decode, Query)]
#[value(Interned<[Savable]>)]
#[extend(name = get_boxing_edges, by_val)]
pub struct BoxingEdges {
    /// The coroutine whose frame has the slots.
    pub instance: MonoCoroutineInstance,
}

#[executor(config = Config)]
async fn boxing_edges_executor(key: &BoxingEdges, engine: &TrackedEngine) -> Interned<[Savable]> {
    // a frame that holds no other frame is on no cycle, and needs no search
    if engine.get_frame_successors(key.instance.clone()).await.is_empty() {
        return engine.intern_unsized(Vec::new());
    }

    let Some(representative) = engine.get_scc_representative(key.instance.clone()).await else {
        return engine.intern_unsized(Vec::new());
    };

    let boxed = engine.break_recursion(representative).await;
    boxed.get(&key.instance).cloned().unwrap_or_else(|| engine.intern_unsized(Vec::new()))
}

#[distributed_slice(RAY_PROGRAM)]
static BOXING_EDGES_EXECUTOR: Registration<Config> =
    Registration::new::<BoxingEdges, BoxingEdgesExecutor>();

/// The part of the frame graph that one coroutine reaches, with the slots of
/// every coroutine in it.
struct ReachableGraph {
    /// The coroutines, in the order they were found, the start first.
    nodes: Vec<MonoCoroutineInstance>,
    numbers: FxHashMap<MonoCoroutineInstance, usize>,
    slots: Vec<Interned<[FrameSlot]>>,
}

impl ReachableGraph {
    /// Finds every coroutine that `start` reaches, reading nothing but their
    /// slots.
    async fn explore(start: MonoCoroutineInstance, engine: &TrackedEngine) -> Self {
        let mut graph =
            Self { nodes: Vec::new(), numbers: FxHashMap::default(), slots: Vec::new() };
        graph.insert(start);

        // breadth first: every node found is explored once, in order
        let mut next = 0;
        while let Some(node) = graph.nodes.get(next).cloned() {
            let slots = engine.get_frame_successors(node).await;
            for target in slots.iter().flat_map(FrameSlot::targets) {
                if !graph.numbers.contains_key(target) {
                    graph.insert(target.clone());
                }
            }
            graph.slots.push(slots);
            next += 1;
        }

        graph
    }

    fn insert(&mut self, node: MonoCoroutineInstance) {
        self.numbers.insert(node.clone(), self.nodes.len());
        self.nodes.push(node);
    }

    /// The members of the strongly connected component of the start, in
    /// `Ord` order: the coroutines it reaches that also reach it.
    ///
    /// Every node already is one the start reaches, so the walk backward from
    /// the start finds the members. It walks only the slots of those nodes,
    /// and that misses no member: every coroutine on a path from a member back
    /// to the start is reached from the start too, through that member.
    fn component_of_start(&self) -> Vec<MonoCoroutineInstance> {
        // walk the slots backward from the start
        let mut predecessors = vec![Vec::new(); self.nodes.len()];
        for (owner, slots) in self.slots.iter().enumerate() {
            for target in slots.iter().flat_map(FrameSlot::targets) {
                predecessors[self.numbers[target]].push(owner);
            }
        }

        let mut reaches_start = BitSet::new(self.nodes.len());
        reaches_start.set(0);
        let mut pending = vec![0];
        while let Some(node) = pending.pop() {
            for &predecessor in &predecessors[node] {
                if reaches_start.set(predecessor) {
                    pending.push(predecessor);
                }
            }
        }

        // every node is reached from the start, by construction
        let mut members = self
            .nodes
            .iter()
            .enumerate()
            .filter(|(number, _)| reaches_start.get(*number))
            .map(|(_, node)| node.clone())
            .collect::<Vec<_>>();
        members.sort_unstable();
        members
    }

    /// The slots of `node`.
    fn slots_of(&self, node: &MonoCoroutineInstance) -> impl Iterator<Item = &FrameSlot> {
        self.slots[self.numbers[node]].iter()
    }

    /// Whether a slot of `node` holds `node` itself.
    fn holds_itself(&self, node: &MonoCoroutineInstance) -> bool {
        self.slots_of(node).any(|slot| slot.targets().any(|target| target == node))
    }
}
