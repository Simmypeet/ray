use bon::Builder;
use qbice::{Decode, Encode, Identifiable, StableHash};

use crate::{layout::VariantIndex, operand::Operand, place::Place, rvalue::Rvalue};

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct Assign {
    destination: Place,
    value: Rvalue,
}

impl Assign {
    #[must_use]
    pub const fn new(destination: Place, value: Rvalue) -> Self { Self { destination, value } }

    #[must_use]
    pub const fn destination(&self) -> &Place { &self.destination }

    #[must_use]
    pub const fn value(&self) -> &Rvalue { &self.value }
}

/// A call at a precise position in a basic block.
///
/// A function operand denotes a direct call. Copying a function pointer from a
/// place denotes an indirect call. Calls returning `void` have no destination.
///
/// The arguments follow the slots of the callee's [`FunctionSignature`]: the
/// environment, the source arguments, then one handler record per effect.
///
/// [`FunctionSignature`]: crate::ty::FunctionSignature
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    StableHash,
    Encode,
    Decode,
    Identifiable,
    Builder,
)]
pub struct Call {
    destination: Option<Place>,
    callee: Operand,
    environment: Option<Operand>,
    arguments: Vec<Operand>,
    #[builder(default = Vec::new())]
    handler_records: Vec<Operand>,
}

impl Call {
    #[must_use]
    pub const fn destination(&self) -> Option<&Place> { self.destination.as_ref() }

    #[must_use]
    pub const fn callee(&self) -> &Operand { &self.callee }

    #[must_use]
    pub const fn environment(&self) -> Option<&Operand> { self.environment.as_ref() }

    /// The source arguments, without the environment and the handler records.
    #[must_use]
    pub fn arguments(&self) -> &[Operand] { &self.arguments }

    #[must_use]
    pub fn handler_records(&self) -> &[Operand] { &self.handler_records }

    /// Every argument in C order: the environment, the source arguments, then
    /// the handler records.
    pub fn abi_arguments(&self) -> impl Iterator<Item = &Operand> {
        self.environment.iter().chain(&self.arguments).chain(&self.handler_records)
    }
}

/// Makes the enum at a place hold one of its variants.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct SetDiscriminant {
    place: Place,
    variant: VariantIndex,
}

impl SetDiscriminant {
    #[must_use]
    pub const fn new(place: Place, variant: VariantIndex) -> Self { Self { place, variant } }

    /// The enum whose variant is set.
    #[must_use]
    pub const fn place(&self) -> &Place { &self.place }

    /// The variant the enum holds from now on.
    #[must_use]
    pub const fn variant(&self) -> VariantIndex { self.variant }
}

/// An operation evaluated in sequence within a basic block.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub enum Instruction {
    Assign(Assign),
    Call(Call),

    /// Releases the heap memory that an [`Rvalue::HeapAllocate`] gave, through
    /// the pointer it gave.
    ///
    /// Only the memory is released: a value still stored in it is not
    /// dropped.
    HeapFree(Operand),

    /// Makes the enum at a place hold one of its variants, whose payload, if
    /// any, has been written through [`Projection::VariantPayload`] already:
    /// a backend may store the discriminant in bytes of the payload.
    ///
    /// [`Projection::VariantPayload`]: crate::place::Projection::VariantPayload
    SetDiscriminant(SetDiscriminant),
}
