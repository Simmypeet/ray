use qbice::{Decode, Encode, StableHash};
use rayc_qbice::TrackedEngine;
use rayc_type::subst::{MutSubstitutable, Subst};

use crate::{
    statement::Statement,
    typed_expr::{SubExprs, TypedExprID, errored::ErroredChild},
};

/// A branch of an expression that chooses between several, such as an arm of
/// an `if` or a clause of a `handle`: either one expression, or a block of
/// statements.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum Arm {
    Expression(TypedExprID),
    Block(Vec<Statement>),
}

impl Arm {
    #[must_use]
    pub const fn expression(&self) -> Option<TypedExprID> {
        match self {
            Self::Expression(expression) => Some(*expression),
            Self::Block(_) => None,
        }
    }

    /// Returns whether control never leaves the arm through its end: its
    /// block ends in a `break`, a `continue` or a `return`.
    ///
    /// This is syntactic. A block that diverges in any other way, such as
    /// through an `if` whose arms all return, is not recognized.
    #[must_use]
    pub fn diverges(&self) -> bool {
        match self {
            Self::Expression(_) => false,
            Self::Block(statements) => statements.last().is_some_and(Statement::diverges),
        }
    }

    pub(crate) fn sub_exprs(&self) -> impl Iterator<Item = TypedExprID> {
        let expression = match self {
            Self::Expression(expression) => Some(*expression),
            Self::Block(_) => None,
        };
        let statements = match self {
            Self::Expression(_) => [].as_slice(),
            Self::Block(statements) => statements.as_slice(),
        };

        expression.into_iter().chain(statements.iter().flat_map(SubExprs::sub_exprs))
    }

    /// Turns the arm into the children an errored expression keeps of it:
    /// its expression, or the statements of its block.
    pub fn into_errored_children(self) -> impl Iterator<Item = ErroredChild> {
        let (expression, statements) = match self {
            Self::Expression(expression) => (Some(expression), Vec::new()),
            Self::Block(statements) => (None, statements),
        };

        expression
            .into_iter()
            .map(ErroredChild::from)
            .chain(statements.into_iter().map(ErroredChild::from))
    }
}

impl MutSubstitutable for Arm {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        match self {
            Self::Expression(_) => {}
            Self::Block(statements) => {
                for statement in statements {
                    statement.apply_mut_subst(subst, engine);
                }
            }
        }
    }
}
