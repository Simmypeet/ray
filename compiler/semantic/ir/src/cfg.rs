use std::ops::Index;

use bon::Builder;
use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_arena::{Arena, ID};
use rayc_hash::FxHashSet;
use rayc_lexical::tree::RelativeSpan;
use rayc_type::ty::Ty;

use crate::{
    address::Address,
    ir_expr::IRExprID,
    scope::ScopeID,
    visit::{
        TypeSite, TypeVisitor, TypeVisitorMut, TypeVisitorMutAsync, VisitType, VisitTypeMut,
        VisitTypeMutAsync,
    },
};

mod instruction;
mod terminator;
mod traversal;

pub use instruction::{AddressDrop, ExprDiscard, Instruction, InstructionInsertion, Store};
pub use terminator::{
    Conditional, ControlFlowEdge, ControlFlowEdgeKind, Handle, HandleClause, Suspend, Terminator,
};
pub use traversal::{BoundaryBlocks, IncomingEdges, OutgoingEdges, Reachables, Traverser};

/// Identifies a basic block stored in a function's control-flow graph.
pub type BlockID = ID<Block>;

/// Identifies a specific instruction in the control-flow graph of a function,
/// by its block and position in that block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Builder)]
pub struct Point {
    block_id: BlockID,
    instruction_idx: usize,
}

impl Point {
    #[must_use]
    pub const fn block_id(&self) -> BlockID { self.block_id }

    #[must_use]
    pub const fn instruction_idx(&self) -> usize { self.instruction_idx }
}

/// A basic block whose instructions execute in insertion order.
///
/// Expression instructions define their value exactly once at their position in
/// the block. An expression that may suspend is evaluated by the terminator
/// instead; see [`Suspend`]. Non-phi operands must already be defined on every
/// path reaching that instruction. Phi operands are instead defined on their
/// corresponding incoming predecessor. Store instructions consume an
/// already-defined value and perform their write at their position in the
/// block. Expression discard instructions drop an evaluated value that is
/// otherwise unused. A block is sealed when its single terminator is set and
/// cannot then be changed or extended.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Default)]
pub struct Block {
    predecessors: FxHashSet<BlockID>,
    instructions: Vec<Instruction>,
    terminator: Option<Terminator>,
}

impl Block {
    #[must_use]
    pub const fn terminator(&self) -> Option<&Terminator> { self.terminator.as_ref() }
}

/// A function's control-flow graph.
///
/// Every block reachable from the entry block is expected to have exactly one
/// terminator whose successors identify blocks in this graph.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub struct Cfg {
    blocks: Arena<Block>,
    entry_block: ID<Block>,
}

impl Index<BlockID> for Cfg {
    type Output = Block;

    fn index(&self, index: BlockID) -> &Self::Output {
        self.blocks.get(index).expect("Block should exist")
    }
}

impl Default for Cfg {
    fn default() -> Self { Self::new() }
}

impl VisitType for Cfg {
    fn visit_types<V: TypeVisitor>(&self, site: TypeSite, visitor: &mut V) {
        for (_, block) in self.blocks.iter() {
            for instruction in &block.instructions {
                instruction.visit_types(site, visitor);
            }
        }
    }
}

impl VisitTypeMut for Cfg {
    fn visit_types_mut<V: TypeVisitorMut>(&mut self, site: TypeSite, visitor: &mut V) {
        for (_, block) in self.blocks.iter_mut() {
            for instruction in &mut block.instructions {
                instruction.visit_types_mut(site, visitor);
            }
        }
    }
}

impl VisitTypeMutAsync for Cfg {
    async fn visit_types_mut_async<V: TypeVisitorMutAsync>(
        &mut self,
        site: TypeSite,
        visitor: &mut V,
    ) {
        for (_, block) in self.blocks.iter_mut() {
            for instruction in &mut block.instructions {
                instruction.visit_types_mut_async(site, visitor).await;
            }
        }
    }
}

impl Cfg {
    #[must_use]
    pub fn new() -> Self {
        let mut blocks = Arena::new();
        let entry_block = blocks.insert(Block::default());
        Self { blocks, entry_block }
    }

    pub fn fill_return_on_unterminated_blocks(&mut self) {
        self.blocks
            .iter_mut()
            .filter(|(_, block)| block.terminator.is_none())
            .for_each(|(_, block)| block.terminator = Some(Terminator::Return(None)));
    }

    #[must_use]
    pub const fn entry_block(&self) -> BlockID { self.entry_block }

    #[must_use]
    pub fn create_block(&mut self) -> BlockID { self.blocks.insert(Block::default()) }

    pub fn push_expression(&mut self, block_id: BlockID, expression: IRExprID) {
        self.push_instruction(block_id, Instruction::Expression(expression));
    }

    pub fn push_expr_discard(
        &mut self,
        block_id: BlockID,
        expression: IRExprID,
        drop_instance: Interned<Ty>,
    ) {
        self.push_instruction(
            block_id,
            Instruction::ExprDiscard(ExprDiscard { expression, drop_instance }),
        );
    }

    pub fn push_scope_push_instruction(&mut self, block_id: BlockID, scope_id: ScopeID) {
        self.push_instruction(block_id, Instruction::ScopePush(scope_id));
    }

    pub fn push_scope_pop_instruction(&mut self, block_id: BlockID, scope_id: ScopeID) {
        self.push_instruction(block_id, Instruction::ScopePop(scope_id));
    }

    fn push_instruction(&mut self, block_id: BlockID, instruction: Instruction) {
        let block = self.blocks.get_mut(block_id).expect("Block should exist");
        assert!(block.terminator.is_none(), "Cannot append an instruction to a sealed block");
        block.instructions.push(instruction);
    }

    pub fn push_store(
        &mut self,
        block_id: BlockID,
        address: Address,
        expression: IRExprID,
        span: RelativeSpan,
    ) {
        let block = self.blocks.get_mut(block_id).expect("Block should exist");
        assert!(block.terminator.is_none(), "Cannot append an instruction to a sealed block");
        block.instructions.push(Instruction::Store(Store { address, expression, span }));
    }

    /// Applies every instruction queued in `insertion`.
    ///
    /// # Panics
    ///
    /// Panics if a queued point names a missing block or lies past the end of
    /// it.
    pub fn insert_instructions(&mut self, insertion: InstructionInsertion) {
        for (block_id, queued) in insertion.blocks {
            let block = self.blocks.get_mut(block_id).expect("Block should exist");
            assert!(
                queued.last_key_value().is_none_or(|(index, _)| *index <= block.instructions.len()),
                "insertion point is past the end of its block"
            );

            // Each block's queue is ordered by index, so splicing from the
            // highest index down leaves every lower index pointing into the
            // original layout.
            for (index, instructions) in queued.into_iter().rev() {
                block.instructions.splice(index..index, instructions);
            }
        }
    }

    pub fn set_terminator(&mut self, block_id: BlockID, terminator: Terminator) {
        // set the predecessors of the successor blocks to include this block
        for target in terminator.jump_targets() {
            self.blocks[target].predecessors.insert(block_id);
        }

        let block = self.blocks.get_mut(block_id).expect("Block should exist");
        assert!(block.terminator.is_none(), "Cannot set a terminator on a sealed block");
        block.terminator = Some(terminator);
    }

    /// Splits every critical edge in the graph and returns the number of
    /// intermediate blocks that were inserted.
    ///
    /// An edge is critical when its source has multiple outgoing edges and its
    /// target has multiple incoming edges. Each such edge is replaced by an
    /// edge to a new empty block that unconditionally jumps to the original
    /// target.
    ///
    /// Like [`Self::split_edge`], this leaves the phis of each target
    /// pointing at the old source, so only the graph's own tests use it.
    #[cfg(test)]
    pub(crate) fn split_critical_edges(&mut self) -> usize {
        let mut critical_edges = Vec::new();

        // Take a snapshot before mutating the graph so every original critical
        // edge is split exactly once.
        for block_id in self.blocks.iter().map(|(block_id, _)| block_id) {
            let outgoing_edges = self.outgoing_edges(block_id).expect("Block should exist");

            if outgoing_edges.clone().count() < 2 {
                continue;
            }

            for edge in outgoing_edges {
                let incoming_edge_count =
                    self.incoming_edges(edge.target).expect("Target block should exist").count();

                if incoming_edge_count >= 2 {
                    critical_edges.push(edge);
                }
            }
        }

        // Redirect each critical edge through a fresh block. Edge counts, not
        // predecessor-block counts, are used above because both arms of a
        // conditional may target the same block.
        for edge in critical_edges.iter().copied() {
            let _ = self.split_edge(edge);
        }

        critical_edges.len()
    }

    /// Redirects `edge` through a new empty block that unconditionally jumps
    /// to the original target, and returns the new block.
    ///
    /// Instructions placed in the new block run only when control follows
    /// this edge.
    ///
    /// The graph holds no expressions, so the phis of the target are left
    /// pointing at the old source. Use [`IRFunction::split_edge`], which
    /// rewires them, outside of this crate.
    ///
    /// [`IRFunction::split_edge`]: crate::ir_function::IRFunction::split_edge
    pub(crate) fn split_edge(&mut self, edge: ControlFlowEdge) -> BlockID {
        let split_block = self.create_block();
        self.set_terminator(split_block, Terminator::Jump(edge.target));
        self.redirect_edge(edge, split_block);
        split_block
    }

    fn redirect_edge(&mut self, edge: ControlFlowEdge, new_target: BlockID) {
        // Update the selected terminator arm and determine whether another arm
        // still connects the original source and target.
        let source = self.blocks.get_mut(edge.source).expect("Source block should exist");
        let terminator = source.terminator.as_mut().expect("Source block should be sealed");
        let target = match (edge.kind, &mut *terminator) {
            (ControlFlowEdgeKind::Jump, Terminator::Jump(target)) => target,
            (ControlFlowEdgeKind::ConditionalTrue, Terminator::Conditional(conditional)) => {
                &mut conditional.true_block
            }
            (ControlFlowEdgeKind::ConditionalFalse, Terminator::Conditional(conditional)) => {
                &mut conditional.false_block
            }
            (ControlFlowEdgeKind::SuspendResume, Terminator::Suspend(suspend)) => {
                &mut suspend.resume
            }
            (ControlFlowEdgeKind::SuspendAbandon, Terminator::Suspend(suspend)) => {
                suspend.abandon.as_mut().expect("Abandon edge should have an abandon block")
            }
            (ControlFlowEdgeKind::HandleReturn, Terminator::Handle(handle)) => {
                &mut handle.return_block
            }
            (ControlFlowEdgeKind::HandleClause(operation_id), Terminator::Handle(handle)) => {
                handle.clause_block_mut(operation_id)
            }
            (ControlFlowEdgeKind::HandleAbandon, Terminator::Handle(handle)) => {
                handle.abandon.as_mut().expect("Abandon edge should have an abandon block")
            }

            (
                ControlFlowEdgeKind::Jump
                | ControlFlowEdgeKind::ConditionalTrue
                | ControlFlowEdgeKind::ConditionalFalse
                | ControlFlowEdgeKind::SuspendResume
                | ControlFlowEdgeKind::SuspendAbandon
                | ControlFlowEdgeKind::HandleReturn
                | ControlFlowEdgeKind::HandleClause(_)
                | ControlFlowEdgeKind::HandleAbandon,
                Terminator::Jump(_)
                | Terminator::Conditional(_)
                | Terminator::Return(_)
                | Terminator::Suspend(_)
                | Terminator::Handle(_),
            ) => panic!("Edge kind should match its terminator"),
        };
        assert_eq!(*target, edge.target, "Edge target should match its terminator");
        *target = new_target;
        let still_targets_original = terminator.jump_targets().any(|target| target == edge.target);

        // Keep the cached predecessor sets consistent with the rewritten
        // terminator, including parallel edges between the same two blocks.
        self.blocks[new_target].predecessors.insert(edge.source);
        if !still_targets_original {
            self.blocks[edge.target].predecessors.remove(&edge.source);
        }
    }

    #[must_use]
    pub fn instructions_with_points(
        &self,
        block_id: BlockID,
    ) -> impl ExactSizeIterator<Item = (Point, &Instruction)> {
        self.blocks.get(block_id).expect("Block should exist").instructions.iter().enumerate().map(
            move |(instruction_idx, instruction)| {
                (Point { block_id, instruction_idx }, instruction)
            },
        )
    }

    #[must_use]
    pub fn instructions_with_points_rev(
        &self,
        block_id: BlockID,
    ) -> impl ExactSizeIterator<Item = (Point, &Instruction)> {
        self.blocks
            .get(block_id)
            .expect("Block should exist")
            .instructions
            .iter()
            .enumerate()
            .rev()
            .map(move |(instruction_idx, instruction)| {
                (Point { block_id, instruction_idx }, instruction)
            })
    }

    /// Iterates over the points control reaches right after `point`: the next
    /// instruction of its block, or, from its terminator, the first point of
    /// each block it jumps to.
    ///
    /// A point one past the last instruction of a block stands for its
    /// terminator.
    pub fn successor_points(&self, point: Point) -> impl Iterator<Item = Point> + '_ {
        let Point { block_id, instruction_idx } = point;
        let is_terminator = instruction_idx == self.instructions(block_id).len();

        let next =
            (!is_terminator).then(|| Point { block_id, instruction_idx: instruction_idx + 1 });
        let jumps = is_terminator
            .then(|| self.terminator(block_id))
            .flatten()
            .into_iter()
            .flat_map(Terminator::jump_targets)
            .map(|target| Point { block_id: target, instruction_idx: 0 });

        next.into_iter().chain(jumps)
    }

    /// Iterates over the points control comes from right before `point`: the
    /// previous instruction of its block, or, from its first point, the
    /// terminator of each predecessor block.
    ///
    /// A point one past the last instruction of a block stands for its
    /// terminator.
    pub fn predecessor_points(&self, point: Point) -> impl Iterator<Item = Point> + '_ {
        let Point { block_id, instruction_idx } = point;

        let previous = instruction_idx
            .checked_sub(1)
            .map(|instruction_idx| Point { block_id, instruction_idx });
        let jumps =
            (instruction_idx == 0).then(|| &self[block_id].predecessors).into_iter().flatten().map(
                |&predecessor| Point {
                    block_id: predecessor,
                    instruction_idx: self.instructions(predecessor).len(),
                },
            );

        previous.into_iter().chain(jumps)
    }

    #[must_use]
    pub fn instructions(&self, block_id: BlockID) -> &[Instruction] {
        &self.blocks.get(block_id).expect("Block should exist").instructions
    }

    #[must_use]
    pub fn terminator(&self, block_id: BlockID) -> Option<&Terminator> {
        self.blocks.get(block_id).expect("Block should exist").terminator.as_ref()
    }

    /// Iterates over blocks that do not have a terminator.
    pub fn unterminated_blocks(&self) -> impl Iterator<Item = BlockID> + '_ {
        self.blocks
            .iter()
            .filter_map(|(block_id, block)| block.terminator.is_none().then_some(block_id))
    }
}

#[cfg(test)]
mod tests;
