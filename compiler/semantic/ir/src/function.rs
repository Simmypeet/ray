use qbice::{Decode, Encode, StableHash};

use crate::{
    address::Address,
    cfg::{BlockID, Cfg, Instruction, Terminator, ValidationError},
    expression::{Expression, ExpressionID, ExpressionMap},
    variable::{Variable, VariableID, VariableMap},
};

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Default)]
pub struct Function {
    cfg: Cfg,
    variable_map: VariableMap,
    expression_map: ExpressionMap,
}

impl Function {
    #[must_use]
    pub fn new() -> Self { Self::default() }

    #[must_use]
    pub fn get_expression(&self, id: ExpressionID) -> &Expression {
        self.expression_map.get_expression(id)
    }

    #[must_use]
    pub fn get_variable(&self, id: VariableID) -> &Variable { self.variable_map.get_variable(id) }

    #[must_use]
    pub const fn entry_block(&self) -> BlockID { self.cfg.entry_block() }

    #[must_use]
    pub fn create_block(&mut self) -> BlockID { self.cfg.create_block() }

    #[must_use]
    pub fn insert_expression(&mut self, expression: Expression) -> ExpressionID {
        self.expression_map.insert_expression(expression)
    }

    #[must_use]
    pub fn insert_variable(&mut self, variable: Variable) -> VariableID {
        self.variable_map.insert_variable(variable)
    }

    pub fn push_expression(&mut self, block_id: BlockID, expression: ExpressionID) {
        self.cfg.push_expression(block_id, expression);
    }

    pub fn push_store(&mut self, block_id: BlockID, address: Address, value: ExpressionID) {
        self.cfg.push_store(block_id, address, value);
    }

    pub fn set_terminator(&mut self, block_id: BlockID, terminator: Terminator) {
        self.cfg.set_terminator(block_id, terminator);
    }

    #[must_use]
    pub fn block_instructions(&self, block_id: BlockID) -> &[Instruction] {
        self.cfg.instructions(block_id)
    }

    #[must_use]
    pub fn block_terminator(&self, block_id: BlockID) -> Option<&Terminator> {
        self.cfg.terminator(block_id)
    }

    pub fn validate(&self) -> Result<(), ValidationError> { self.cfg.validate() }
}
