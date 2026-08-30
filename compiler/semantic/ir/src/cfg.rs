use std::collections::VecDeque;

use qbice::{Decode, Encode, StableHash};
use rayc_arena::{Arena, ID};
use rayc_hash::FxHashSet;

use crate::{address::Address, ir_expr::IRExprID};

/// Identifies a basic block stored in a function's control-flow graph.
pub type BlockID = ID<Block>;

/// A basic block whose instructions execute in insertion order.
///
/// Expression instructions define their value exactly once at their position in
/// the block. Non-phi operands must already be defined on every path reaching
/// that instruction. Phi operands are instead defined on their corresponding
/// incoming predecessor. Store instructions consume an already-defined value
/// and perform their write at their position in the block. A block is sealed
/// when its single terminator is set and cannot then be changed or extended.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Default)]
pub struct Block {
    instructions: Vec<Instruction>,
    terminator: Option<Terminator>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct Store {
    address: Address,
    expression: IRExprID,
}

impl Store {
    #[must_use]
    pub const fn address(&self) -> &Address { &self.address }

    #[must_use]
    pub const fn expression(&self) -> IRExprID { self.expression }
}

/// An operation evaluated at a precise position in a basic block.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub enum Instruction {
    /// Defines and evaluates the identified expression exactly once.
    Expression(IRExprID),
    /// Writes an already-defined expression value to an address.
    Store(Store),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Conditional {
    condition: IRExprID,
    then_block: BlockID,
    else_block: BlockID,
}

impl Conditional {
    #[must_use]
    pub const fn new(condition: IRExprID, then_block: BlockID, else_block: BlockID) -> Self {
        Self { condition, then_block, else_block }
    }

    #[must_use]
    pub const fn condition(&self) -> IRExprID { self.condition }

    #[must_use]
    pub const fn then_block(&self) -> BlockID { self.then_block }

    #[must_use]
    pub const fn else_block(&self) -> BlockID { self.else_block }
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

/// The blocks and expression instructions reachable from a control-flow
/// graph's entry block, in breadth-first visit order.
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

impl Default for Cfg {
    fn default() -> Self { Self::new() }
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
        let block = self.blocks.get_mut(block_id).expect("Block should exist");
        assert!(block.terminator.is_none(), "Cannot append an instruction to a sealed block");
        block.instructions.push(Instruction::Expression(expression));
    }

    pub fn push_store(&mut self, block_id: BlockID, address: Address, expression: IRExprID) {
        let block = self.blocks.get_mut(block_id).expect("Block should exist");
        assert!(block.terminator.is_none(), "Cannot append an instruction to a sealed block");
        block.instructions.push(Instruction::Store(Store { address, expression }));
    }

    pub fn set_terminator(&mut self, block_id: BlockID, terminator: Terminator) {
        let block = self.blocks.get_mut(block_id).expect("Block should exist");
        assert!(block.terminator.is_none(), "Cannot replace a block terminator");
        block.terminator = Some(terminator);
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
                if let Instruction::Expression(expression_id) = instruction
                    && visited_expressions.insert(*expression_id)
                {
                    reachable_expressions.push(*expression_id);
                }
            }
            let terminator =
                block.terminator.as_ref().expect("Reachable block should have a terminator");

            match terminator {
                Terminator::Jump(successor) => {
                    pending.push_back(*successor);
                }
                Terminator::Conditional(conditional) => {
                    pending.push_back(conditional.then_block);
                    pending.push_back(conditional.else_block);
                }
                Terminator::Return(_) => {}
            }
        }

        Reachables { reachable_blocks, reachable_expressions }
    }
}
