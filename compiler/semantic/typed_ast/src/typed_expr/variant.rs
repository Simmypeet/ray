use qbice::{Decode, Encode, StableHash};
use rayc_symbol::GlobalSymbolID;

use crate::typed_expr::{SubExprs, TypedExprID};

/// Constructs a value of an enum from one of its variants: `Option.None`, or
/// `Option.Some(value)` for a variant that carries a payload.
///
/// The expression has the type of the enum, which carries its type arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Variant {
    variant_id: GlobalSymbolID,
    payload: Option<TypedExprID>,
}

impl Variant {
    /// Creates the construction of the `SymbolKind::Variant` symbol
    /// `variant_id`, with `payload` when the variant carries one.
    #[must_use]
    pub const fn new(variant_id: GlobalSymbolID, payload: Option<TypedExprID>) -> Self {
        Self { variant_id, payload }
    }

    /// Returns the `SymbolKind::Variant` symbol being constructed.
    #[must_use]
    pub const fn variant_id(&self) -> GlobalSymbolID { self.variant_id }

    /// Returns the payload of the variant, if it carries one.
    #[must_use]
    pub const fn payload(&self) -> Option<TypedExprID> { self.payload }
}

impl SubExprs for Variant {
    fn sub_exprs(&self) -> impl Iterator<Item = TypedExprID> { self.payload.into_iter() }
}
