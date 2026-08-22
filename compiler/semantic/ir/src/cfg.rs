use qbice::{Decode, Encode, StableHash};
use rayc_arena::{Arena, ID};
use rayc_hash::FxHashSet;

use crate::{address::Address, expression::ExpressionID};

/// Identifies a basic block stored in a function's control-flow graph.
pub type BlockID = ID<Block>;

/// A basic block whose instructions execute in insertion order.
///
/// Expression instructions define their value exactly once at their position in
/// the block. All operands must already be defined on every path reaching that
/// instruction. Store instructions consume an already-defined value and perform
/// their write at their position in the block. A block is sealed when its
/// single terminator is set and cannot then be changed or extended.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Default)]
pub struct Block {
    instructions: Vec<Instruction>,
    terminator: Option<Terminator>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct Store {
    address: Address,
    expression: ExpressionID,
}

impl Store {
    #[must_use]
    pub const fn address(&self) -> &Address { &self.address }

    #[must_use]
    pub const fn expression(&self) -> ExpressionID { self.expression }
}

/// An operation evaluated at a precise position in a basic block.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub enum Instruction {
    /// Defines and evaluates the identified expression exactly once.
    Expression(ExpressionID),
    /// Writes an already-defined expression value to an address.
    Store(Store),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Conditional {
    condition: ExpressionID,
    then_block: BlockID,
    else_block: BlockID,
}

impl Conditional {
    #[must_use]
    pub const fn new(condition: ExpressionID, then_block: BlockID, else_block: BlockID) -> Self {
        Self { condition, then_block, else_block }
    }

    #[must_use]
    pub const fn condition(&self) -> ExpressionID { self.condition }

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
    Return(Option<ExpressionID>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationError {
    InvalidEntryBlock(BlockID),
    InvalidSuccessor { block: BlockID, successor: BlockID },
    UnterminatedBlock(BlockID),
}

/// A function's control-flow graph.
///
/// Every block reachable from the entry block must have exactly one terminator
/// before the graph is considered complete. [`Cfg::validate`] checks this
/// requirement and verifies that reachable successors identify blocks in this
/// graph.
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

    #[must_use]
    pub const fn entry_block(&self) -> BlockID { self.entry_block }

    #[must_use]
    pub fn create_block(&mut self) -> BlockID { self.blocks.insert(Block::default()) }

    pub fn push_expression(&mut self, block_id: BlockID, expression: ExpressionID) {
        let block = self.blocks.get_mut(block_id).expect("Block should exist");
        assert!(block.terminator.is_none(), "Cannot append an instruction to a sealed block");
        block.instructions.push(Instruction::Expression(expression));
    }

    pub fn push_store(&mut self, block_id: BlockID, address: Address, expression: ExpressionID) {
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

    /// Checks the structural invariants required of a completed graph.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if !self.blocks.contains_id(self.entry_block) {
            return Err(ValidationError::InvalidEntryBlock(self.entry_block));
        }

        let mut pending = vec![self.entry_block];
        let mut visited = FxHashSet::default();
        while let Some(block_id) = pending.pop() {
            if !visited.insert(block_id) {
                continue;
            }

            let block = self.blocks.get(block_id).expect("Visited block should exist");
            let terminator =
                block.terminator.as_ref().ok_or(ValidationError::UnterminatedBlock(block_id))?;

            match terminator {
                Terminator::Jump(successor) => {
                    self.validate_successor(block_id, *successor)?;
                    pending.push(*successor);
                }
                Terminator::Conditional(conditional) => {
                    self.validate_successor(block_id, conditional.then_block)?;
                    self.validate_successor(block_id, conditional.else_block)?;
                    pending.push(conditional.then_block);
                    pending.push(conditional.else_block);
                }
                Terminator::Return(_) => {}
            }
        }

        Ok(())
    }

    fn validate_successor(
        &self,
        block: BlockID,
        successor: BlockID,
    ) -> Result<(), ValidationError> {
        if self.blocks.contains_id(successor) {
            Ok(())
        } else {
            Err(ValidationError::InvalidSuccessor { block, successor })
        }
    }
}
