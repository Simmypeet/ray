use std::collections::{VecDeque, hash_set};

use rayc_arena::ID;
use rayc_hash::FxHashSet;

use super::{Block, BlockID, Cfg, ControlFlowEdge, ControlFlowEdgeKind, Instruction, Terminator};
use crate::{dataflow::Direction, ir_expr::IRExprID};

/// An iterator for traversing through the control flow graph.
///
/// Every block is guaranteed to be visited exactly once and reachable from
/// the entry block.
///
/// The iterator is called in a depth-first and pre-order manner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Traverser<'a> {
    cfg: &'a Cfg,
    visited: FxHashSet<BlockID>,
    stack: Vec<BlockID>,
}

impl<'a> Iterator for Traverser<'a> {
    type Item = (ID<Block>, &'a Block);

    fn next(&mut self) -> Option<Self::Item> {
        let block_id = loop {
            let block_id = self.stack.pop()?;
            if self.visited.insert(block_id) {
                break block_id;
            }
        };

        let block = &self.cfg[block_id];

        self.stack.extend(block.terminator().iter().flat_map(|x| x.jump_targets()));

        Some((block_id, block))
    }
}

/// The blocks and expressions reachable from a control-flow graph's entry
/// block, in breadth-first visit order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reachables {
    reachable_blocks: Vec<BlockID>,
    reachable_expressions: Vec<IRExprID>,
}

impl Reachables {
    #[must_use]
    pub fn blocks(&self) -> impl ExactSizeIterator<Item = BlockID> + '_ {
        self.reachable_blocks.iter().copied()
    }

    #[must_use]
    pub fn expressions(&self) -> impl ExactSizeIterator<Item = IRExprID> + '_ {
        self.reachable_expressions.iter().copied()
    }
}

#[derive(Debug, Clone)]
enum OutgoingEdgeCursor {
    Empty,
    Unconditional(Option<ControlFlowEdge>),
    Conditional {
        true_edge: Option<ControlFlowEdge>,
        false_edge: Option<ControlFlowEdge>,
    },
    Suspend {
        resume_edge: Option<ControlFlowEdge>,
        abandon_edge: Option<ControlFlowEdge>,
    },
    Handle {
        return_edge: Option<ControlFlowEdge>,
        clause_edges: std::vec::IntoIter<ControlFlowEdge>,
        abandon_edge: Option<ControlFlowEdge>,
    },
}

/// Iterates over the outgoing edges of a block. Only the edges of a
/// [`Terminator::Handle`], which has any number of clauses, are allocated on
/// the heap.
#[derive(Debug, Clone)]
pub struct OutgoingEdges {
    cursor: OutgoingEdgeCursor,
}

impl Iterator for OutgoingEdges {
    type Item = ControlFlowEdge;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.cursor {
            OutgoingEdgeCursor::Empty => None,

            OutgoingEdgeCursor::Unconditional(edge) => edge.take(),

            OutgoingEdgeCursor::Conditional { true_edge, false_edge } => {
                true_edge.take().or_else(|| false_edge.take())
            }

            OutgoingEdgeCursor::Suspend { resume_edge, abandon_edge } => {
                resume_edge.take().or_else(|| abandon_edge.take())
            }

            OutgoingEdgeCursor::Handle { return_edge, clause_edges, abandon_edge } => {
                return_edge.take().or_else(|| clause_edges.next()).or_else(|| abandon_edge.take())
            }
        }
    }
}

/// Iterates over the incoming edges of a block without heap allocation.
#[derive(Debug, Clone)]
pub struct IncomingEdges<'a> {
    graph: &'a Cfg,
    target: ID<Block>,
    predecessors: hash_set::Iter<'a, BlockID>,
    current_outgoing: Option<OutgoingEdges>,
}

impl Iterator for IncomingEdges<'_> {
    type Item = ControlFlowEdge;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(edge) = self.current_outgoing.as_mut().and_then(Iterator::next) {
                if edge.target == self.target {
                    return Some(edge);
                }

                continue;
            }

            let predecessor = *self.predecessors.next()?;
            self.current_outgoing = self.graph.outgoing_edges(predecessor);
        }
    }
}

/// Iterates over the boundary blocks for a given dataflow direction.
#[derive(Debug, Clone)]
pub struct BoundaryBlocks<'a> {
    graph: &'a Cfg,
    direction: Direction,
    traverser: Traverser<'a>,
    emitted_entry: bool,
}

impl Iterator for BoundaryBlocks<'_> {
    type Item = ID<Block>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.direction {
            Direction::Forward => {
                if self.emitted_entry {
                    None
                } else {
                    self.emitted_entry = true;
                    Some(self.graph.entry_block)
                }
            }

            Direction::Backward => self.traverser.find_map(|(block_id, _)| {
                self.graph.outgoing_edges(block_id)?.next().is_none().then_some(block_id)
            }),
        }
    }
}

impl Cfg {
    /// Iterates over reachable blocks.
    #[must_use]
    pub fn traverse(&self) -> Traverser<'_> {
        Traverser { cfg: self, visited: FxHashSet::default(), stack: vec![self.entry_block] }
    }

    /// Returns the blocks reachable from the entry block in reverse postorder:
    /// every block precedes its successors, except along back edges.
    ///
    /// The depth-first search visits the successors of a block in the order of
    /// its terminator's jump targets.
    #[must_use]
    pub fn reverse_postorder(&self) -> Vec<BlockID> {
        let mut visited = FxHashSet::default();
        let mut postorder = Vec::new();

        // Each frame holds a block and its successors not yet visited, stored
        // reversed so the next one is popped from the end.
        visited.insert(self.entry_block);
        let mut stack = vec![(self.entry_block, self.successors_reversed(self.entry_block))];
        while let Some((block_id, successors)) = stack.last_mut() {
            if let Some(successor) = successors.pop() {
                if visited.insert(successor) {
                    let successors = self.successors_reversed(successor);
                    stack.push((successor, successors));
                }
            } else {
                postorder.push(*block_id);
                stack.pop();
            }
        }

        postorder.reverse();
        postorder
    }

    fn successors_reversed(&self, block_id: BlockID) -> Vec<BlockID> {
        let mut successors = self
            .terminator(block_id)
            .into_iter()
            .flat_map(Terminator::jump_targets)
            .collect::<Vec<_>>();
        successors.reverse();
        successors
    }

    /// Calculates the blocks and expression instructions reachable from the
    /// entry block.
    #[must_use]
    pub fn reachables(&self) -> Reachables {
        let mut pending = VecDeque::from([self.entry_block]);
        let mut visited_blocks = FxHashSet::default();
        let mut visited_expressions = FxHashSet::default();
        let mut reachable_blocks = Vec::new();
        let mut reachable_expressions = Vec::new();
        while let Some(block_id) = pending.pop_front() {
            if !visited_blocks.insert(block_id) {
                continue;
            }
            reachable_blocks.push(block_id);

            let block = self.blocks.get(block_id).expect("Reachable block should exist");
            for instruction in &block.instructions {
                match instruction {
                    Instruction::ScopePush(_)
                    | Instruction::ScopePop(_)
                    | Instruction::AddressDrop(_)
                    | Instruction::Store(_) => {}
                    Instruction::ExprDiscard(discard) => {
                        if visited_expressions.insert(discard.expression) {
                            reachable_expressions.push(discard.expression);
                        }
                    }
                    Instruction::Expression(expression_id) => {
                        if visited_expressions.insert(*expression_id) {
                            reachable_expressions.push(*expression_id);
                        }
                    }
                }
            }
            let terminator =
                block.terminator.as_ref().expect("Reachable block should have a terminator");

            // A terminator evaluates its expressions after the instructions
            // of its block.
            for expression_id in terminator.evaluated_expressions() {
                if visited_expressions.insert(expression_id) {
                    reachable_expressions.push(expression_id);
                }
            }

            pending.extend(terminator.jump_targets());
        }

        Reachables { reachable_blocks, reachable_expressions }
    }

    /// Returns the outgoing edges from the given block.
    #[must_use]
    pub fn outgoing_edges(&self, block_id: ID<Block>) -> Option<OutgoingEdges> {
        let block = self.blocks.get(block_id)?;

        let cursor = match block.terminator() {
            None | Some(Terminator::Return(_)) => OutgoingEdgeCursor::Empty,

            Some(Terminator::Jump(unconditional)) => {
                OutgoingEdgeCursor::Unconditional(Some(ControlFlowEdge {
                    source: block_id,
                    target: *unconditional,
                    kind: ControlFlowEdgeKind::Jump,
                }))
            }

            Some(Terminator::Conditional(conditional)) => OutgoingEdgeCursor::Conditional {
                true_edge: Some(ControlFlowEdge {
                    source: block_id,
                    target: conditional.true_block,
                    kind: ControlFlowEdgeKind::ConditionalTrue,
                }),
                false_edge: Some(ControlFlowEdge {
                    source: block_id,
                    target: conditional.false_block,
                    kind: ControlFlowEdgeKind::ConditionalFalse,
                }),
            },

            Some(Terminator::Suspend(suspend)) => OutgoingEdgeCursor::Suspend {
                resume_edge: Some(ControlFlowEdge {
                    source: block_id,
                    target: suspend.resume,
                    kind: ControlFlowEdgeKind::SuspendResume,
                }),
                abandon_edge: suspend.abandon.map(|abandon| ControlFlowEdge {
                    source: block_id,
                    target: abandon,
                    kind: ControlFlowEdgeKind::SuspendAbandon,
                }),
            },

            Some(Terminator::Handle(handle)) => OutgoingEdgeCursor::Handle {
                return_edge: Some(ControlFlowEdge {
                    source: block_id,
                    target: handle.return_block,
                    kind: ControlFlowEdgeKind::HandleReturn,
                }),
                clause_edges: handle
                    .clauses()
                    .map(|(operation_id, clause)| ControlFlowEdge {
                        source: block_id,
                        target: clause.block,
                        kind: ControlFlowEdgeKind::HandleClause(operation_id),
                    })
                    .collect::<Vec<_>>()
                    .into_iter(),
                abandon_edge: handle.abandon.map(|abandon| ControlFlowEdge {
                    source: block_id,
                    target: abandon,
                    kind: ControlFlowEdgeKind::HandleAbandon,
                }),
            },
        };

        Some(OutgoingEdges { cursor })
    }

    /// Returns the incoming edges to the given block.
    #[must_use]
    pub fn incoming_edges(&self, block_id: ID<Block>) -> Option<IncomingEdges<'_>> {
        let block = self.blocks.get(block_id)?;

        Some(IncomingEdges {
            graph: self,
            target: block_id,
            predecessors: block.predecessors.iter(),
            current_outgoing: None,
        })
    }

    /// Returns the boundary blocks for the given dataflow direction.
    #[must_use]
    pub fn boundary_block_ids(&self, direction: Direction) -> BoundaryBlocks<'_> {
        BoundaryBlocks { graph: self, direction, traverser: self.traverse(), emitted_entry: false }
    }
}
