use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_hash::FxHashMap;
use rayc_symbol::GlobalSymbolID;

use crate::{
    MonoEffectInstance,
    operand::Operand,
    place::Place,
    ty::{
        Closure as ClosureTy, Environment as EnvironmentTy, MonoType, PointerMutability,
        Tuple as TupleTy,
    },
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
pub struct AggregateTuple {
    ty: TupleTy,
    fields: Vec<Operand>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct AggregateEnvironment {
    ty: EnvironmentTy,
    fields: Vec<Operand>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct AggregateClosure {
    ty: ClosureTy,
    environment: Operand,
    function: Operand,
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct OperationHandlerSlot {
    environment: Operand,
    function: Operand,
}

impl OperationHandlerSlot {
    #[must_use]
    pub const fn new(environment: Operand, function: Operand) -> Self {
        Self { environment, function }
    }

    #[must_use]
    pub const fn environment(&self) -> &Operand { &self.environment }

    #[must_use]
    pub const fn function(&self) -> &Operand { &self.function }
}

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct AggregateEffectHandler {
    effect: MonoEffectInstance,
    slots: FxHashMap<GlobalSymbolID, OperationHandlerSlot>,
}

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub enum AggregateValue {
    Tuple(AggregateTuple),
    Environment(AggregateEnvironment),
    Closure(AggregateClosure),
    EffectHandler(AggregateEffectHandler),
}

/// A pure, shallow value computation.
///
/// Operands cannot contain nested computations. Calls are instructions rather
/// than rvalues so evaluation order is always explicit in the CFG.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub enum Rvalue {
    Use(Operand),
    AddressOf(AddressOf),
    Unary(Unary),
    Binary(Binary),
    Cast(Cast),
    Aggregate(AggregateValue),
}

impl Rvalue {
    #[must_use]
    pub fn new_tuple(ty: TupleTy, fields: Vec<Operand>) -> Self {
        assert_eq!(ty.len(), fields.len());

        Self::Aggregate(AggregateValue::Tuple(AggregateTuple { ty, fields }))
    }

    #[must_use]
    pub const fn new_closure(ty: ClosureTy, environment: Operand, function: Operand) -> Self {
        Self::Aggregate(AggregateValue::Closure(AggregateClosure { ty, environment, function }))
    }

    #[must_use]
    pub const fn new_environment(ty: EnvironmentTy, fields: Vec<Operand>) -> Self {
        Self::Aggregate(AggregateValue::Environment(AggregateEnvironment { ty, fields }))
    }

    #[must_use]
    pub const fn new_effect_handler(
        effect: MonoEffectInstance,
        slots: FxHashMap<GlobalSymbolID, OperationHandlerSlot>,
    ) -> Self {
        Self::Aggregate(AggregateValue::EffectHandler(AggregateEffectHandler { effect, slots }))
    }
}
