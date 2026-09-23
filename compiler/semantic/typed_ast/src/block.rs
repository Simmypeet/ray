use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_qbice::TrackedEngine;
use rayc_type::{
    subst::{MutSubstitutable, Subst, Substitutable},
    ty::Ty,
};

use crate::statement::Statement;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Block {
    statements: Vec<Statement>,
    effect: Interned<Ty>,
}

impl Block {
    #[must_use]
    pub(super) const fn new(effect: Interned<Ty>) -> Self {
        Self { statements: Vec::new(), effect }
    }

    pub fn statements(&self) -> impl Iterator<Item = &Statement> { self.statements.iter() }

    #[must_use]
    pub const fn effect(&self) -> &Interned<Ty> { &self.effect }

    pub fn push_statement(&mut self, statement: Statement) { self.statements.push(statement); }
}

impl MutSubstitutable for Block {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        self.effect.apply_in_place(subst, engine);
        for statement in &mut self.statements {
            statement.apply_mut_subst(subst, engine);
        }
    }
}
