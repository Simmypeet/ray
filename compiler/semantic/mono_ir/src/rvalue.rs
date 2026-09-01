use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};

use crate::{
    operand::Operand,
    place::Place,
    ty::{MonoType, PointerMutability},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum UnaryOperator {
    Negate,
    LogicalNot,
    BitwiseNot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum BinaryOperator {
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    LogicalAnd,
    LogicalOr,
    BitwiseAnd,
    BitwiseOr,
    BitwiseXor,
    ShiftLeft,
    ShiftRight,
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct AddressOf {
    place: Place,
    mutability: PointerMutability,
}

impl AddressOf {
    #[must_use]
    pub const fn new(place: Place, mutability: PointerMutability) -> Self {
        Self { place, mutability }
    }

    #[must_use]
    pub const fn place(&self) -> &Place { &self.place }

    #[must_use]
    pub const fn mutability(&self) -> PointerMutability { self.mutability }
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct Unary {
    operator: UnaryOperator,
    operand: Operand,
}

impl Unary {
    #[must_use]
    pub const fn new(operator: UnaryOperator, operand: Operand) -> Self {
        Self { operator, operand }
    }

    #[must_use]
    pub const fn operator(&self) -> UnaryOperator { self.operator }

    #[must_use]
    pub const fn operand(&self) -> &Operand { &self.operand }
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct Binary {
    left: Operand,
    operator: BinaryOperator,
    right: Operand,
}

impl Binary {
    #[must_use]
    pub const fn new(left: Operand, operator: BinaryOperator, right: Operand) -> Self {
        Self { left, operator, right }
    }

    #[must_use]
    pub const fn left(&self) -> &Operand { &self.left }

    #[must_use]
    pub const fn operator(&self) -> BinaryOperator { self.operator }

    #[must_use]
    pub const fn right(&self) -> &Operand { &self.right }
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct Cast {
    operand: Operand,
    target: Interned<MonoType>,
}

impl Cast {
    #[must_use]
    pub const fn new(operand: Operand, target: Interned<MonoType>) -> Self {
        Self { operand, target }
    }

    #[must_use]
    pub const fn operand(&self) -> &Operand { &self.operand }

    #[must_use]
    pub const fn target(&self) -> &Interned<MonoType> { &self.target }
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct AggregateValue {
    ty: Interned<MonoType>,
    fields: Vec<Operand>,
}

impl AggregateValue {
    #[must_use]
    pub const fn new(ty: Interned<MonoType>, fields: Vec<Operand>) -> Self { Self { ty, fields } }

    #[must_use]
    pub const fn ty(&self) -> &Interned<MonoType> { &self.ty }

    #[must_use]
    pub fn fields(&self) -> &[Operand] { &self.fields }
}

/// A pure, shallow value computation.
///
/// Operands cannot contain nested computations. Calls are instructions rather
/// than rvalues so evaluation order is always explicit in the CFG.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum Rvalue {
    Use(Operand),
    AddressOf(AddressOf),
    Unary(Unary),
    Binary(Binary),
    Cast(Cast),
    Aggregate(AggregateValue),
}
