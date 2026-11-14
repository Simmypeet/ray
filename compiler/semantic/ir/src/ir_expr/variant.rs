use qbice::{Decode, Encode, StableHash};
use rayc_symbol::GlobalSymbolID;

use crate::ir_expr::IRExprID;

/// Constructs a value of an enum from one of its variants, moving the payload
/// into it when the variant carries one.
///
/// The expression has the type of the enum, which carries its type arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, StableHash, Encode, Decode)]
pub struct Variant {
    variant_id: GlobalSymbolID,
    payload: Option<IRExprID>,
}

impl Variant {
    /// Creates the construction of the `SymbolKind::Variant` symbol
    /// `variant_id`, with `payload` when the variant carries one.
    #[must_use]
    pub const fn new(variant_id: GlobalSymbolID, payload: Option<IRExprID>) -> Self {
        Self { variant_id, payload }
    }

    /// Returns the `SymbolKind::Variant` symbol being constructed.
    #[must_use]
    pub const fn variant_id(&self) -> GlobalSymbolID { self.variant_id }

    /// Returns the payload of the variant, if it carries one.
    #[must_use]
    pub const fn payload(&self) -> Option<IRExprID> { self.payload }
}
