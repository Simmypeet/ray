use qbice::{Decode, Encode, StableHash};

use crate::{
    statement::Statement,
    typed_expr::{SubExprs, TypedExprID},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum ErroredChild {
    Expression(TypedExprID),
    Statement(Statement),
}

impl From<TypedExprID> for ErroredChild {
    fn from(expression: TypedExprID) -> Self { Self::Expression(expression) }
}

impl From<Statement> for ErroredChild {
    fn from(statement: Statement) -> Self { Self::Statement(statement) }
}

impl ErroredChild {
    fn sub_exprs(&self) -> impl Iterator<Item = TypedExprID> {
        let expression = match self {
            Self::Expression(expression) => Some(*expression),
            Self::Statement(_) => None,
        };
        let statement = match self {
            Self::Expression(_) => None,
            Self::Statement(statement) => Some(statement),
        };

        expression.into_iter().chain(statement.into_iter().flat_map(SubExprs::sub_exprs))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Errored {
    children: Vec<ErroredChild>,
}

impl Errored {
    #[must_use]
    pub const fn new_empty() -> Self { Self { children: Vec::new() } }

    #[must_use]
    pub const fn new(children: Vec<ErroredChild>) -> Self { Self { children } }

    #[must_use]
    pub fn children(&self) -> &[ErroredChild] { &self.children }
}

impl SubExprs for Errored {
    fn sub_exprs(&self) -> impl Iterator<Item = TypedExprID> {
        self.children.iter().flat_map(ErroredChild::sub_exprs)
    }
}
