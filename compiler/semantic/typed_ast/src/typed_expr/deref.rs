use qbice::{Decode, Encode, StableHash};

use crate::typed_expr::{SubExprs, TypedExprID};

/// What a dereference goes through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum DerefKind {
    /// A checked reference.
    Reference,

    /// An unchecked raw pointer. The place behind it is untracked memory.
    /// This is also the kind of a dereference whose operand failed to type.
    RawPointer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Deref {
    pointee: TypedExprID,
    kind: DerefKind,
}

impl Deref {
    #[must_use]
    pub const fn new(pointee: TypedExprID, kind: DerefKind) -> Self { Self { pointee, kind } }

    #[must_use]
    pub const fn pointee(&self) -> TypedExprID { self.pointee }

    /// Returns what the dereference goes through. It is decided when the
    /// dereference is typed, since later passes may only see the operand's
    /// type as an unreduced projection.
    #[must_use]
    pub const fn kind(&self) -> DerefKind { self.kind }
}

impl SubExprs for Deref {
    fn sub_exprs(&self) -> impl Iterator<Item = TypedExprID> { std::iter::once(self.pointee) }
}
