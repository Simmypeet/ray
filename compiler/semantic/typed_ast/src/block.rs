use qbice::{Decode, Encode, StableHash};

use crate::statement::Statement;

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Default,
)]
pub struct Block {
    statements: Vec<Statement>,
}

impl Block {
    pub fn push_statement(&mut self, statement: Statement) { self.statements.push(statement); }
}
