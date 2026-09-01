use qbice::{Decode, Encode, Identifiable, StableHash};
use rayc_arena::{Arena, ID};

use crate::{instruction::Instruction, operand::Operand};

pub type BlockID = ID<BasicBlock>;

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct Branch {
    condition: Operand,
    then_block: BlockID,
    else_block: BlockID,
}

impl Branch {
    #[must_use]
    pub const fn new(condition: Operand, then_block: BlockID, else_block: BlockID) -> Self {
        Self { condition, then_block, else_block }
    }

    #[must_use]
    pub const fn condition(&self) -> &Operand { &self.condition }

    #[must_use]
    pub const fn then_block(&self) -> BlockID { self.then_block }

    #[must_use]
    pub const fn else_block(&self) -> BlockID { self.else_block }
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum Terminator {
    Goto(BlockID),
    Branch(Branch),
    Return(Option<Operand>),
    Unreachable,
}

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Default, Identifiable)]
pub struct BasicBlock {
    instructions: Vec<Instruction>,
    terminator: Option<Terminator>,
}

impl BasicBlock {
    #[must_use]
    pub fn instructions(&self) -> &[Instruction] { &self.instructions }

    #[must_use]
    pub const fn terminator(&self) -> Option<&Terminator> { self.terminator.as_ref() }
}

/// A function's C-like control-flow graph.
///
/// Every reachable block is expected to have one terminator. Phi nodes are not
/// part of `MonoIR`; lowering represents merge values with locals and explicit
/// assignments on incoming control-flow edges.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct MonoCfg {
    blocks: Arena<BasicBlock>,
    entry: BlockID,
}

impl Default for MonoCfg {
    fn default() -> Self { Self::new() }
}

impl MonoCfg {
    #[must_use]
    pub fn new() -> Self {
        let mut blocks = Arena::new();
        let entry = blocks.insert(BasicBlock::default());
        Self { blocks, entry }
    }

    #[must_use]
    pub const fn entry(&self) -> BlockID { self.entry }

    #[must_use]
    pub fn create_block(&mut self) -> BlockID { self.blocks.insert(BasicBlock::default()) }

    pub fn push_instruction(&mut self, block_id: BlockID, instruction: Instruction) {
        let block = self.blocks.get_mut(block_id).expect("MonoIR block should exist");
        assert!(block.terminator.is_none(), "cannot append to a terminated MonoIR block");
        block.instructions.push(instruction);
    }

    pub fn set_terminator(&mut self, block_id: BlockID, terminator: Terminator) {
        let block = self.blocks.get_mut(block_id).expect("MonoIR block should exist");
        assert!(block.terminator.is_none(), "cannot replace a MonoIR block terminator");
        block.terminator = Some(terminator);
    }

    #[must_use]
    pub fn get_block(&self, block_id: BlockID) -> &BasicBlock {
        self.blocks.get(block_id).expect("MonoIR block should exist")
    }

    #[must_use]
    pub fn blocks(&self) -> impl ExactSizeIterator<Item = (BlockID, &BasicBlock)> {
        self.blocks.iter()
    }
}
