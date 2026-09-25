use qbice::{Decode, Encode, StableHash};
use rayc_qbice::TrackedEngine;
use rayc_type::subst::{MutSubstitutable, Subst};

use crate::{
    statement::Statement,
    typed_expr::{SubExprs, TypedExprID},
};

/// A lexical block of statements used as an expression, such as the block of
/// `unsafe:`. Its value is unit.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct StatementBlock {
    statements: Vec<Statement>,
}

impl StatementBlock {
    #[must_use]
    pub const fn new(statements: Vec<Statement>) -> Self { Self { statements } }

    pub fn statements(&self) -> impl Iterator<Item = &Statement> { self.statements.iter() }
}

impl MutSubstitutable for StatementBlock {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        for statement in &mut self.statements {
            statement.apply_mut_subst(subst, engine);
        }
    }
}

impl SubExprs for StatementBlock {
    fn sub_exprs(&self) -> impl Iterator<Item = TypedExprID> {
        self.statements.iter().flat_map(SubExprs::sub_exprs)
    }
}
