use qbice::{Decode, Encode, StableHash};
use rayc_semantic_element::struct_body::FieldID;
use rayc_symbol::GlobalSymbolID;

use crate::typed_expr::{SubExprs, TypedExprID};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct FieldInitializer {
    field: FieldID,
    expression: TypedExprID,
}

impl FieldInitializer {
    #[must_use]
    pub const fn new(field: FieldID, expression: TypedExprID) -> Self { Self { field, expression } }

    #[must_use]
    pub const fn field(&self) -> FieldID { self.field }

    #[must_use]
    pub const fn expression(&self) -> TypedExprID { self.expression }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct StructInitialization {
    struct_id: GlobalSymbolID,

    /// List of field initializers, **in the order they were written in the
    /// source code.**
    initializers: Vec<FieldInitializer>,
}

impl StructInitialization {
    #[must_use]
    pub const fn new(struct_id: GlobalSymbolID, initializers: Vec<FieldInitializer>) -> Self {
        Self { struct_id, initializers }
    }

    #[must_use]
    pub const fn struct_id(&self) -> GlobalSymbolID { self.struct_id }

    #[must_use]
    pub fn initializers(&self) -> &[FieldInitializer] { &self.initializers }
}

impl SubExprs for StructInitialization {
    fn sub_exprs(&self) -> impl Iterator<Item = TypedExprID> {
        self.initializers.iter().map(FieldInitializer::expression)
    }
}
