use qbice::{Decode, Encode, StableHash};
use rayc_hash::FxHashMap;
use rayc_semantic_element::struct_body::FieldID;
use rayc_symbol::GlobalSymbolID;

use crate::ir_expr::IRExprID;

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub struct StructInitialization {
    struct_id: GlobalSymbolID,
    initializers: FxHashMap<FieldID, IRExprID>,
}

impl StructInitialization {
    #[must_use]
    pub const fn new(
        struct_id: GlobalSymbolID,
        initializers: FxHashMap<FieldID, IRExprID>,
    ) -> Self {
        Self { struct_id, initializers }
    }

    #[must_use]
    pub const fn struct_id(&self) -> GlobalSymbolID { self.struct_id }

    #[must_use]
    pub const fn initializers(&self) -> &FxHashMap<FieldID, IRExprID> { &self.initializers }
}
