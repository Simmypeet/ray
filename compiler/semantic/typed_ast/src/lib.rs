use qbice::{Decode, Encode, Identifiable, Query, StableHash, storage::intern::Interned};
use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    subst::{MutSubstitutable, Subst},
    ty::application::ClosureID,
};

use crate::{capture_plan::CapturePlan, typed_function::TypedFunctionMap};

pub mod block;
pub mod capture_plan;
pub mod irrefutable_pattern;
pub mod name_binding;
pub mod statement;
pub mod typed_expr;
pub mod typed_function;
pub mod typed_lambda;
pub mod typed_operation_handler;
pub mod typed_thunk;
pub mod typed_variable;

/// A typed AST and the capture layouts inferred for its nested functions.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct TypedAst {
    functions: TypedFunctionMap,
    captures: CapturePlan,
}

impl TypedAst {
    #[must_use]
    pub const fn new(functions: TypedFunctionMap, captures: CapturePlan) -> Self {
        Self { functions, captures }
    }

    #[must_use]
    pub const fn functions(&self) -> &TypedFunctionMap { &self.functions }

    #[must_use]
    pub const fn captures(&self) -> &CapturePlan { &self.captures }

    #[must_use]
    pub fn closure_function(
        &self,
        closure_id: ClosureID,
    ) -> Option<typed_function::TypedFunctionID> {
        self.functions.closure_function(closure_id)
    }
}

impl MutSubstitutable for TypedAst {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        self.functions.apply_mut_subst(subst, engine);
        self.captures.apply_mut_subst(subst, engine);
    }
}

/// Retrieves the typed AST for a given def ID.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<TypedAst>)]
#[extend(by_val, name = get_typed_ast)]
pub struct Key {
    pub def_id: GlobalSymbolID,
}
