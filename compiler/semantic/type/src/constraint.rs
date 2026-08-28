use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_qbice::TrackedEngine;

use crate::{
    constraint::subtype::Subtype,
    reduce::Reduce,
    solver::Solver,
    subst::{Subst, Substitutable},
    ty::Ty,
};

pub mod subtype;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum Constraint {
    Subtype(Subtype),
}

impl Constraint {
    #[must_use]
    pub const fn new_subtype(lesser: Interned<Ty>, greater: Interned<Ty>) -> Self {
        Self::Subtype(Subtype::new(lesser, greater))
    }
}

impl Reduce for Constraint {
    fn reduce(&self, engine: &TrackedEngine) -> Option<Self>
    where
        Self: Sized,
    {
        match self {
            Self::Subtype(subtype) => subtype.reduce(engine).map(Constraint::Subtype),
        }
    }
}

impl Substitutable for Constraint {
    fn apply_subst(&self, subst: &Subst, engine: &TrackedEngine) -> Option<Self>
    where
        Self: Sized,
    {
        match self {
            Self::Subtype(subtype) => subtype.apply_subst(subst, engine).map(Constraint::Subtype),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DerivationRule {
    TypeApplicationMatching,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DerivedConstraint {
    pub rule: DerivationRule,
    pub constraint: Constraint,
}

impl DerivedConstraint {
    #[must_use]
    pub const fn new(rule: DerivationRule, constraint: Constraint) -> Self {
        Self { rule, constraint }
    }

    #[must_use]
    pub const fn new_type_application_matching(
        lesser: Interned<Ty>,
        greater: Interned<Ty>,
    ) -> Self {
        Self::new(DerivationRule::TypeApplicationMatching, Constraint::new_subtype(lesser, greater))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Step {
    /// A new substitution has been generated
    Subst(Subst),

    /// The constraint has been simplified to a set of new constraints
    Derived(Vec<DerivedConstraint>),

    /// No applicable rules could be found
    NoProgress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Error {
    /// The subtype constraint is obviously unsatisfiable, e.g. `Int <: Bool`
    Conflicted,

    /// The subtype constraint is unsatisfiable due to a cycle, e.g. `T <: T`
    OccursCheckFailed,
}

impl Solver {
    pub fn entail(&mut self, constraint: &Constraint) -> Result<Step, Error> {
        match constraint {
            Constraint::Subtype(subtype) => self.entail_subtype(subtype),
        }
    }
}
