use qbice::{Decode, Encode, StableHash};

use crate::typed_expr::{SubExprs, TypedExprID};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum BinaryOp {
    Assign,
    Equal,
    NotEqual,
    Plus,
    Minus,
    Multiply,
    Divide,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Associativity {
    Left,
    Right,
}

impl BinaryOp {
    #[must_use]
    pub const fn precedence(self) -> u8 {
        match self {
            Self::Assign => 0,
            Self::Or => 1,
            Self::And => 2,
            Self::Equal | Self::NotEqual => 3,
            Self::Plus | Self::Minus => 4,
            Self::Multiply | Self::Divide => 5,
        }
    }

    #[must_use]
    pub const fn associativity(self) -> Associativity {
        match self {
            Self::Assign => Associativity::Right,
            Self::Equal
            | Self::NotEqual
            | Self::Plus
            | Self::Minus
            | Self::Multiply
            | Self::Divide
            | Self::And
            | Self::Or => Associativity::Left,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Binary {
    left: TypedExprID,
    operator: BinaryOp,
    right: TypedExprID,
}

impl Binary {
    #[must_use]
    pub const fn new(left: TypedExprID, operator: BinaryOp, right: TypedExprID) -> Self {
        Self { left, operator, right }
    }

    #[must_use]
    pub const fn left(&self) -> TypedExprID { self.left }

    #[must_use]
    pub const fn operator(&self) -> BinaryOp { self.operator }

    #[must_use]
    pub const fn right(&self) -> TypedExprID { self.right }
}

impl SubExprs for Binary {
    fn sub_exprs(&self) -> impl Iterator<Item = TypedExprID> { [self.left, self.right].into_iter() }
}
