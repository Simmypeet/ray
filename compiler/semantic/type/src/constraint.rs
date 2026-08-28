use qbice::{Decode, Encode, StableHash};
use rayc_qbice::TrackedEngine;

use crate::{
    constraint::subtype::Subtype,
    reduce::Reduce,
    solver::Solver,
    subst::{Subst, Substitutable},
};

pub mod subtype;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum Constraint {
    Subtype(Subtype),
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Step {
    /// A new substitution has been generated
    Subst(Subst),

    /// The constraint has been simplified to a set of new constraints
    Derived(Vec<Constraint>),

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
