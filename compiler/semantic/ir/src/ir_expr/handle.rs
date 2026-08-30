use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_symbol::GlobalSymbolID;
use rayc_type::{subst::Subst, ty::Ty};

use crate::{
    ir_expr::IRExprID,
    ir_function::FunctionID,
    visit::{TypeVisitor, VisitType},
};

/// A nested function paired with the capture operands used to create its
/// closure.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct HandledFunction {
    function_id: FunctionID,
    captures: Vec<IRExprID>,
}

impl HandledFunction {
    #[must_use]
    pub const fn new(function_id: FunctionID, captures: Vec<IRExprID>) -> Self {
        Self { function_id, captures }
    }

    #[must_use]
    pub const fn function_id(&self) -> FunctionID { self.function_id }

    #[must_use]
    pub fn captures(&self) -> &[IRExprID] { &self.captures }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct OperationHandler {
    operation_id: GlobalSymbolID,
    function: HandledFunction,
}

impl OperationHandler {
    #[must_use]
    pub const fn new(operation_id: GlobalSymbolID, function: HandledFunction) -> Self {
        Self { operation_id, function }
    }

    #[must_use]
    pub const fn operation_id(&self) -> GlobalSymbolID { self.operation_id }

    #[must_use]
    pub const fn function(&self) -> &HandledFunction { &self.function }
}

/// Runs a thunk under a complete set of handlers for one instantiated effect.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct Handle {
    effect_id: GlobalSymbolID,
    substitution: Subst,
    body: HandledFunction,
    handlers: Vec<OperationHandler>,
    residual_effect: Interned<Ty>,
}

impl Handle {
    #[must_use]
    pub const fn new(
        effect_id: GlobalSymbolID,
        substitution: Subst,
        body: HandledFunction,
        handlers: Vec<OperationHandler>,
        residual_effect: Interned<Ty>,
    ) -> Self {
        Self { effect_id, substitution, body, handlers, residual_effect }
    }

    #[must_use]
    pub const fn effect_id(&self) -> GlobalSymbolID { self.effect_id }

    #[must_use]
    pub const fn substitution(&self) -> &Subst { &self.substitution }

    #[must_use]
    pub const fn body(&self) -> &HandledFunction { &self.body }

    #[must_use]
    pub fn handlers(&self) -> &[OperationHandler] { &self.handlers }

    #[must_use]
    pub const fn residual_effect(&self) -> &Interned<Ty> { &self.residual_effect }
}

impl VisitType for Handle {
    fn visit_types<V: TypeVisitor>(&self, visitor: &mut V) {
        for ty in self.substitution.codomain() {
            visitor.visit_type(ty);
        }
        visitor.visit_type(&self.residual_effect);
    }
}
