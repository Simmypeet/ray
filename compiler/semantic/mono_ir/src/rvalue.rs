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

/// Reads the bytes of an operand of one type as a value of another type, both
/// of which are spelled because a place alone does not tell its type.
///
/// The types may differ in size. Only the bytes that both have are defined, so
/// what is read must not depend on the others: a `coroutine_resume_t` is read
/// as that of another row through the member they have in common, which starts
/// at the first byte of both.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct Reinterpret {
    operand: Operand,
    source: Interned<MonoType>,
    target: Interned<MonoType>,
}

impl Reinterpret {
    #[must_use]
    pub const fn new(
        operand: Operand,
        source: Interned<MonoType>,
        target: Interned<MonoType>,
    ) -> Self {
        Self { operand, source, target }
    }

    #[must_use]
    pub const fn operand(&self) -> &Operand { &self.operand }

    #[must_use]
    pub const fn source(&self) -> &Interned<MonoType> { &self.source }

    #[must_use]
    pub const fn target(&self) -> &Interned<MonoType> { &self.target }
}

/// A shallow value computation.
///
/// Operands cannot contain nested computations. Calls are instructions rather
/// than rvalues so evaluation order is always explicit in the CFG. Every
/// rvalue is pure except [`Rvalue::HeapAllocate`].
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub enum Rvalue {
    Use(Operand),
    AddressOf(AddressOf),
    Unary(Unary),
    Binary(Binary),
    Cast(Cast),
    Reinterpret(Reinterpret),

    /// Allocates uninitialized heap memory for one value of the given type,
    /// giving a `*mut` pointer to it.
    ///
    /// The memory is released by an [`Instruction::HeapFree`] of that
    /// pointer. Allocation never fails: the program aborts instead.
    ///
    /// [`Instruction::HeapFree`]: crate::instruction::Instruction::HeapFree
    HeapAllocate(Interned<MonoType>),
}
