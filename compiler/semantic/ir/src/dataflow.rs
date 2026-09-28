//! Generic dataflow solving utilities over the control flow graph.

use std::{collections::VecDeque, future::Future};

use rayc_hash::{FxHashMap, FxHashSet};

use crate::cfg::{BlockID, Cfg, ControlFlowEdge, Instruction, Point, Terminator};

/// Represents the direction in which a dataflow problem propagates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// Facts flow from block entry to block exit.
    Forward,

    /// Facts flow from block exit to block entry.
    Backward,
}

/// A join-semilattice used by a dataflow problem.
pub trait JoinLattice<D: ?Sized, E>: Clone + Eq {
    /// Joins `other` into `self`, returning whether `self` changed or the join
    /// failed.
    fn join<'a>(
        &'a mut self,
        other: &'a Self,
        dataflow_problem_ctx: &'a D,
    ) -> impl Future<Output = Result<bool, E>> + Send + use<'a, Self, D, E>;
}

impl<T, D, E> JoinLattice<D, E> for Option<T>
where
    T: JoinLattice<D, E> + Send + Sync,
    D: Sync + ?Sized,
{
    async fn join(&mut self, other: &Self, dataflow_problem_ctx: &D) -> Result<bool, E> {
        match other {
            None => Ok(false),

            Some(other) => match self {
                None => {
                    *self = Some(other.clone());
                    Ok(true)
                }

                Some(this) => this.join(other, dataflow_problem_ctx).await,
            },
        }
    }
}

/// Describes a dataflow problem that can be solved over a CFG.
pub trait DataflowProblem {
    /// The lattice carried through the analysis.
    type JoinLattice: JoinLattice<Self, Self::Error>;

    /// The error returned by transfer or initialization routines.
    type Error;

    /// The propagation direction of the analysis.
    const DIRECTION: Direction;

    /// Determines whether the solver tracks distinct facts for each CFG edge.
    ///
    /// When this is `false`, facts flow directly between neighboring blocks
    /// without invoking [`Self::transfer_edge`], and edge states are omitted
    /// from the final solution to reduce memory usage.
    const EDGE_SENSITIVE: bool;

    /// Creates the lattice bottom used to initialize the given block.
    fn bottom(
        &mut self,
        block_id: BlockID,
    ) -> impl Future<Output = Result<Self::JoinLattice, Self::Error>> + Send + use<'_, Self>;

    /// Creates the facts used to initialize boundary blocks.
    fn boundary_facts(
        &mut self,
        block_id: BlockID,
    ) -> impl Future<Output = Result<Self::JoinLattice, Self::Error>> + Send + use<'_, Self>;

    /// Applies the instruction transfer function in-place.
    fn transfer_instruction<'a>(
        &'a mut self,
        point: Point,
        instruction: &'a Instruction,
        state: &'a mut Self::JoinLattice,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + use<'a, Self>;

    /// Applies the terminator transfer function in-place.
    fn transfer_terminator<'a>(
        &'a mut self,
        block_id: BlockID,
        terminator: &'a Terminator,
        state: &'a mut Self::JoinLattice,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + use<'a, Self>;

    /// Applies the edge transfer function in-place.
    ///
    /// This is invoked only when [`Self::EDGE_SENSITIVE`] is `true`.
    fn transfer_edge<'a>(
        &'a mut self,
        edge: &'a ControlFlowEdge,
        state: &'a mut Self::JoinLattice,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + use<'a, Self>;
}

/// Stores the solved dataflow facts for reachable CFG blocks and edges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataflowSolution<L> {
    edge_sensitive: bool,
    reachable_blocks: Vec<BlockID>,
    edges: Vec<ControlFlowEdge>,
    block_entries: FxHashMap<BlockID, L>,
    block_exits: FxHashMap<BlockID, L>,
    edge_states: Option<FxHashMap<ControlFlowEdge, L>>,
}

impl<L> DataflowSolution<L> {
    /// Returns the solved entry fact for the given block.
    #[must_use]
    pub fn block_entry(&self, block_id: BlockID) -> Option<&L> { self.block_entries.get(&block_id) }

    /// Returns the solved exit fact for the given block.
    #[must_use]
    pub fn block_exit(&self, block_id: BlockID) -> Option<&L> { self.block_exits.get(&block_id) }

    /// Returns the solved state for the given control-flow edge.
    ///
    /// # Panics
    ///
    /// Panics if edge sensitivity was disabled for the solved problem.
    #[must_use]
    pub fn edge_state(&self, edge: &ControlFlowEdge) -> Option<&L> {
        assert!(
            self.edge_sensitive,
            "edge states are unavailable when edge sensitivity is disabled",
        );

        self.edge_states.as_ref().unwrap().get(edge)
    }

    /// Returns the reachable blocks that were solved.
    #[must_use]
    pub fn reachable_blocks(&self) -> impl ExactSizeIterator<Item = BlockID> + '_ {
        self.reachable_blocks.iter().copied()
    }

    /// Returns the reachable edges that were solved.
    #[must_use]
    pub fn edges(&self) -> impl ExactSizeIterator<Item = &'_ ControlFlowEdge> + '_ {
        self.edges.iter()
    }
}

fn enqueue_block(
    worklist: &mut VecDeque<BlockID>,
    queued_blocks: &mut FxHashSet<BlockID>,
    block_id: BlockID,
) {
    if queued_blocks.insert(block_id) {
        worklist.push_back(block_id);
    }
}

/// Solves a dataflow problem to a fixpoint using Kildall's algorithm.
#[allow(clippy::cognitive_complexity, clippy::too_many_lines)]
pub async fn solve<P: DataflowProblem>(
    problem: &mut P,
    cfg: &Cfg,
) -> Result<DataflowSolution<P::JoinLattice>, P::Error> {
    let reachable_blocks = cfg.traverse().map(|(id, _)| id).collect::<Vec<_>>();
    let mut block_entries = FxHashMap::default();
    let mut block_exits = FxHashMap::default();

    let mut edge_states =
        P::EDGE_SENSITIVE.then(FxHashMap::<ControlFlowEdge, P::JoinLattice>::default);
    let mut edges = Vec::new();

    for block_id in reachable_blocks.iter().copied() {
        let bottom = problem.bottom(block_id).await?;
        block_entries.insert(block_id, bottom.clone());
        block_exits.insert(block_id, bottom);
    }

    for block_id in reachable_blocks.iter().copied() {
        for edge in cfg.outgoing_edges(block_id).unwrap() {
            if let Some(edge_states) = &mut edge_states {
                let edge_bottom = match P::DIRECTION {
                    Direction::Forward => block_exits.get(&edge.source()).unwrap().clone(),
                    Direction::Backward => block_entries.get(&edge.target()).unwrap().clone(),
                };

                edge_states.insert(edge, edge_bottom);
            }

            edges.push(edge);
        }
    }

    edges.sort_unstable();

    for block_id in cfg.boundary_block_ids(P::DIRECTION) {
        let boundary_facts = problem.boundary_facts(block_id).await?;

        match P::DIRECTION {
            Direction::Forward => {
                *block_entries.get_mut(&block_id).unwrap() = boundary_facts;
            }
            Direction::Backward => {
                *block_exits.get_mut(&block_id).unwrap() = boundary_facts;
            }
        }
    }

    // Every reachable block is processed at least once, and afterwards only
    // when its incoming facts change. Boundary facts are only where analysis
    // facts enter the graph, not where processing starts: a block no path
    // connects to a boundary, such as a loop without an exit in a backward
    // problem, is still processed from bottom. Blocks are queued in the
    // direction of the flow, so most blocks see the facts of their
    // predecessors in the flow before they are first processed.
    let mut worklist = VecDeque::new();
    let mut queued_blocks = FxHashSet::default();
    let mut visited_blocks = FxHashSet::default();
    let mut seed_order = cfg.reverse_postorder();
    match P::DIRECTION {
        Direction::Forward => {}
        Direction::Backward => seed_order.reverse(),
    }
    for block_id in seed_order {
        enqueue_block(&mut worklist, &mut queued_blocks, block_id);
    }

    while let Some(block_id) = worklist.pop_front() {
        queued_blocks.remove(&block_id);

        // The first visit propagates even when the block's facts stay at
        // bottom, since an edge transfer may still add facts of its own.
        let first_visit = visited_blocks.insert(block_id);

        match P::DIRECTION {
            Direction::Forward => {
                let mut candidate_state = block_entries.get(&block_id).unwrap().clone();

                for (point, instruction) in cfg.instructions_with_points(block_id) {
                    problem.transfer_instruction(point, instruction, &mut candidate_state).await?;
                }

                if let Some(terminator) = cfg[block_id].terminator() {
                    problem.transfer_terminator(block_id, terminator, &mut candidate_state).await?;
                }

                let block_exit = block_exits.get_mut(&block_id).unwrap();
                let block_changed = *block_exit != candidate_state;

                if block_changed {
                    *block_exit = candidate_state.clone();
                }

                if !block_changed && !first_visit {
                    continue;
                }

                for edge in cfg.outgoing_edges(block_id).unwrap() {
                    if let Some(edge_states) = &mut edge_states {
                        let mut edge_state = candidate_state.clone();
                        problem.transfer_edge(&edge, &mut edge_state).await?;

                        let stored_edge_state = edge_states.get_mut(&edge).unwrap();
                        if *stored_edge_state != edge_state {
                            *stored_edge_state = edge_state.clone();

                            if block_entries
                                .get_mut(&edge.target())
                                .unwrap()
                                .join(&edge_state, problem)
                                .await?
                            {
                                enqueue_block(&mut worklist, &mut queued_blocks, edge.target());
                            }
                        }
                    } else if block_entries
                        .get_mut(&edge.target())
                        .unwrap()
                        .join(&candidate_state, problem)
                        .await?
                    {
                        enqueue_block(&mut worklist, &mut queued_blocks, edge.target());
                    }
                }
            }

            Direction::Backward => {
                let mut candidate_state = block_exits.get(&block_id).unwrap().clone();

                if let Some(terminator) = cfg[block_id].terminator() {
                    problem.transfer_terminator(block_id, terminator, &mut candidate_state).await?;
                }

                for (point, instruction) in cfg.instructions_with_points_rev(block_id) {
                    problem.transfer_instruction(point, instruction, &mut candidate_state).await?;
                }

                let block_entry = block_entries.get_mut(&block_id).unwrap();
                let block_changed = *block_entry != candidate_state;

                if block_changed {
                    *block_entry = candidate_state.clone();
                }

                if !block_changed && !first_visit {
                    continue;
                }

                for edge in cfg.incoming_edges(block_id).unwrap() {
                    if let Some(edge_states) = &mut edge_states {
                        let mut edge_state = candidate_state.clone();
                        problem.transfer_edge(&edge, &mut edge_state).await?;

                        let stored_edge_state = edge_states.get_mut(&edge).unwrap();
                        if *stored_edge_state != edge_state {
                            *stored_edge_state = edge_state.clone();

                            if block_exits
                                .get_mut(&edge.source())
                                .unwrap()
                                .join(&edge_state, problem)
                                .await?
                            {
                                enqueue_block(&mut worklist, &mut queued_blocks, edge.source());
                            }
                        }
                    } else if block_exits
                        .get_mut(&edge.source())
                        .unwrap()
                        .join(&candidate_state, problem)
                        .await?
                    {
                        enqueue_block(&mut worklist, &mut queued_blocks, edge.source());
                    }
                }
            }
        }
    }

    Ok(DataflowSolution {
        edge_sensitive: P::EDGE_SENSITIVE,
        reachable_blocks,
        edges,
        block_entries,
        block_exits,
        edge_states,
    })
}
