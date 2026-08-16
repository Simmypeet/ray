use rayc_arena::ID;
use qbice::{Decode, Encode, StableHash};

use crate::{
    address::Address,
    cfg::{Block, Cfg},
    expression::{Expression, ExpressionMap},
    variable::{Variable, VariableMap},
};

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub struct Function {
    cfg: Cfg,
    variable_map: VariableMap,
    expression_map: ExpressionMap,
}

impl Function {
    #[must_use]
    pub fn get_expression(&self, id: ID<Expression>) -> &Expression {
        self.expression_map.get_expression(id)
    }

    #[must_use]
    pub const fn entry_block(&self) -> ID<Block> { self.cfg.entry_block() }

    #[must_use]
    pub fn insert_expression(&mut self, expression: Expression) -> ID<Expression> {
        self.expression_map.insert_expression(expression)
    }

    #[must_use]
    pub fn insert_variable(&mut self, variable: Variable) -> ID<Variable> {
        self.variable_map.insert_variable(variable)
    }

    pub fn push_expression(&mut self, block_id: ID<Block>, expression: ID<Expression>) {
        self.cfg.push_expression(block_id, expression);
    }

    pub fn push_store(&mut self, block_id: ID<Block>, address: Address, value: ID<Expression>) {
        self.cfg.push_store(block_id, address, value);
    }
}
