use rayc_arena::{Arena, ID};
use qbice::{Decode, Encode, StableHash};

use crate::{address::Address, expression::Expression};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Default)]
pub struct Block {
    instructions: Vec<Instruction>,
    terminator: Option<Terminator>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct Store {
    address: Address,
    expression: ID<Expression>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub enum Instruction {
    Expression(ID<Expression>),
    Store(Store),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum Terminator {
    Jump(ID<Block>),
    Return(ID<Expression>),
}

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
    pub const fn entry_block(&self) -> ID<Block> { self.entry_block }

    pub fn push_expression(&mut self, block_id: ID<Block>, expression: ID<Expression>) {
        let block = self.blocks.get_mut(block_id).expect("Block should exist");
        block.instructions.push(Instruction::Expression(expression));
    }

    pub fn push_store(
        &mut self,
        block_id: ID<Block>,
        address: Address,
        expression: ID<Expression>,
    ) {
        let block = self.blocks.get_mut(block_id).expect("Block should exist");
        block.instructions.push(Instruction::Store(Store { address, expression }));
    }
}
