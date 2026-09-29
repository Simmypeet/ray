use std::{
    collections::{BTreeMap, VecDeque, hash_set},
    ops::Index,
};

use bon::Builder;
use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_arena::{Arena, ID};
use rayc_hash::{FxHashMap, FxHashSet};
use rayc_lexical::tree::RelativeSpan;
use rayc_type::ty::Ty;

use crate::{
    address::Address,
    dataflow::Direction,
    ir_expr::IRExprID,
    scope::ScopeID,
    visit::{TypeSite, TypeVisitor, TypeVisitorMut, VisitType, VisitTypeMut},
};

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

/// Instructions queued for insertion into a control-flow graph, applied all
/// at once by [`Cfg::insert_instructions`].
///
/// Every point refers to the block layout before any insertion, so callers
/// can queue insertions while replaying an analysis of that layout.
#[derive(Debug, Default)]
pub struct InstructionInsertion {
    /// The sequences queued in each block, by the index of the instruction
    /// they precede.
    blocks: FxHashMap<BlockID, BTreeMap<usize, Vec<Instruction>>>,
}

impl InstructionInsertion {
    #[must_use]
    pub fn new() -> Self { Self::default() }

    /// Queues `instructions` to run immediately before the instruction
    /// currently at `point`, or at the end of the block when `point` is one
    /// past its last instruction.
    ///
    /// Sequences queued at the same point run in the order they were queued.
    pub fn insert_before(
        &mut self,
        point: Point,
        instructions: impl IntoIterator<Item = Instruction>,
    ) {
        self.blocks
            .entry(point.block_id)
            .or_default()
            .entry(point.instruction_idx)
            .or_default()
            .extend(instructions);
    }
}

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

/// A basic block whose instructions execute in insertion order.
///
/// Expression instructions define their value exactly once at their position in
/// the block. Non-phi operands must already be defined on every path reaching
/// that instruction. Phi operands are instead defined on their corresponding
/// incoming predecessor. Store instructions consume an already-defined value
/// and perform their write at their position in the block. Expression discard
/// instructions drop an evaluated value that is otherwise unused. A block is
/// sealed when its single terminator is set and cannot then be changed or
/// extended.
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct Store {
    address: Address,
    expression: IRExprID,

    /// The source construct performing the write, such as an assignment or a
    /// `let` initializer.
    span: RelativeSpan,
}

impl Store {
    #[must_use]
    pub const fn address(&self) -> &Address { &self.address }

    #[must_use]
    pub const fn expression(&self) -> IRExprID { self.expression }

    #[must_use]
    pub const fn span(&self) -> RelativeSpan { self.span }
}

/// Drops the unused value of an evaluated expression.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct ExprDiscard {
    expression: IRExprID,

    /// The `Drop` dictionary selected for the expression's type.
    drop_instance: Interned<Ty>,
}

impl ExprDiscard {
    #[must_use]
    pub const fn expression(&self) -> IRExprID { self.expression }

    #[must_use]
    pub const fn drop_instance(&self) -> &Interned<Ty> { &self.drop_instance }
}

/// An operation evaluated at a precise position in a basic block.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub enum Instruction {
    /// Begins the lifetime of a lexical or temporary scope.
    ScopePush(ScopeID),
    /// Ends the lifetime of a lexical or temporary scope.
    ///
    /// This is the "storage dead" point of every variable declared in the
    /// scope: drop elaboration drops the values still held there just before
    /// it, and the storage itself is gone after it. A borrow of such a
    /// variable which is still live here is therefore a "borrowed value does
    /// not live long enough" error, or, for the temporaries of a temporary
    /// scope, "temporary value dropped while borrowed". A borrow of the data
    /// behind a pointer held in the variable does not end here.
    ScopePop(ScopeID),
    /// Defines and evaluates the identified expression exactly once.
    Expression(IRExprID),
    /// Drops the unused result of an evaluated expression.
    ExprDiscard(ExprDiscard),
    /// Writes an already-defined expression value to an address.
    Store(Store),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Conditional {
    condition: IRExprID,
    true_block: BlockID,
    false_block: BlockID,
}

impl Conditional {
    #[must_use]
    pub const fn new(condition: IRExprID, true_block: BlockID, false_block: BlockID) -> Self {
        Self { condition, true_block, false_block }
    }

    #[must_use]
    pub const fn condition(&self) -> IRExprID { self.condition }

    #[must_use]
    pub const fn then_block(&self) -> BlockID { self.true_block }

    #[must_use]
    pub const fn else_block(&self) -> BlockID { self.false_block }
}

/// Describes the kind of control-flow edge between two blocks in a control-flow
/// graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum ControlFlowEdgeKind {
    /// A control-flow edge that is always taken.
    Jump,
    /// A control-flow edge that is taken when a condition evaluates to true.
    ConditionalTrue,
    /// A control-flow edge that is taken when a condition evaluates to false.
    ConditionalFalse,
}

/// Describes a control-flow edge between two blocks in a control-flow graph.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Builder,
)]
pub struct ControlFlowEdge {
    kind: ControlFlowEdgeKind,
    source: BlockID,
    target: BlockID,
}

impl ControlFlowEdge {
    #[must_use]
    pub const fn kind(&self) -> ControlFlowEdgeKind { self.kind }

    #[must_use]
    pub const fn source(&self) -> BlockID { self.source }

    #[must_use]
    pub const fn target(&self) -> BlockID { self.target }
}

/// The single operation that transfers control out of a sealed block.
///
/// `Return(None)` is the canonical form of a bare or implicit unit return.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum Terminator {
    Jump(BlockID),
    Conditional(Conditional),
    Return(Option<IRExprID>),
}

impl Terminator {
    pub fn jump_targets(&self) -> impl Iterator<Item = BlockID> + '_ {
        pub enum Iter<A, B, C> {
            A(A),
            B(B),
            C(C),
        }

        impl<A, B, C> Iterator for Iter<A, B, C>
        where
            A: Iterator<Item = BlockID>,
            B: Iterator<Item = BlockID>,
            C: Iterator<Item = BlockID>,
        {
            type Item = BlockID;

            fn next(&mut self) -> Option<Self::Item> {
                match self {
                    Self::A(a) => a.next(),
                    Self::B(b) => b.next(),
                    Self::C(c) => c.next(),
                }
            }
        }

        match self {
            Self::Jump(block_id) => Iter::A(std::iter::once(*block_id)),
            Self::Conditional(conditional) => {
                Iter::B([conditional.true_block, conditional.false_block].into_iter())
            }
            Self::Return(_) => Iter::C(std::iter::empty()),
        }
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

impl VisitType for Instruction {
    fn visit_types<V: TypeVisitor>(&self, site: TypeSite, visitor: &mut V) {
        match self {
            Self::ExprDiscard(discard) => visitor.visit_type(discard.drop_instance(), site),

            Self::ScopePush(_) | Self::ScopePop(_) | Self::Expression(_) | Self::Store(_) => {}
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

impl VisitTypeMut for Instruction {
    fn visit_types_mut<V: TypeVisitorMut>(&mut self, site: TypeSite, visitor: &mut V) {
        match self {
            Self::ExprDiscard(discard) => visitor.visit_type_mut(&mut discard.drop_instance, site),

            Self::ScopePush(_) | Self::ScopePop(_) | Self::Expression(_) | Self::Store(_) => {}
        }
    }
}

#[derive(Debug, Clone)]
enum OutgoingEdgeCursor {
    Empty,
    Unconditional(Option<ControlFlowEdge>),
    Conditional { true_edge: Option<ControlFlowEdge>, false_edge: Option<ControlFlowEdge> },
}

/// Iterates over the outgoing edges of a block without heap allocation.
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
        match &terminator {
            Terminator::Jump(id) => {
                self.blocks[*id].predecessors.insert(block_id);
            }
            Terminator::Conditional(conditional) => {
                self.blocks[conditional.true_block].predecessors.insert(block_id);
                self.blocks[conditional.false_block].predecessors.insert(block_id);
            }
            Terminator::Return(_) => {}
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
        match edge.kind {
            ControlFlowEdgeKind::Jump => match terminator {
                Terminator::Jump(target) => {
                    assert_eq!(*target, edge.target, "Edge target should match its terminator");
                    *target = new_target;
                }
                Terminator::Conditional(_) | Terminator::Return(_) => {
                    panic!("Jump edge should have a jump terminator")
                }
            },
            ControlFlowEdgeKind::ConditionalTrue => match terminator {
                Terminator::Conditional(conditional) => {
                    assert_eq!(
                        conditional.true_block, edge.target,
                        "Edge target should match its terminator"
                    );
                    conditional.true_block = new_target;
                }
                Terminator::Jump(_) | Terminator::Return(_) => {
                    panic!("Conditional edge should have a conditional terminator")
                }
            },
            ControlFlowEdgeKind::ConditionalFalse => match terminator {
                Terminator::Conditional(conditional) => {
                    assert_eq!(
                        conditional.false_block, edge.target,
                        "Edge target should match its terminator"
                    );
                    conditional.false_block = new_target;
                }
                Terminator::Jump(_) | Terminator::Return(_) => {
                    panic!("Conditional edge should have a conditional terminator")
                }
            },
        }
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

            match terminator {
                Terminator::Jump(successor) => {
                    pending.push_back(*successor);
                }
                Terminator::Conditional(conditional) => {
                    pending.push_back(conditional.true_block);
                    pending.push_back(conditional.false_block);
                }
                Terminator::Return(_) => {}
            }
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

#[cfg(test)]
mod tests;
