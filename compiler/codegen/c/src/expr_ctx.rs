use qbice::storage::intern::Interned;
use rayc_type::ty::Ty;
use rayc_typed_ast::{
    function::Function,
    name_binding::{NameBinding, NameBindingID},
    typed_expr::{TypedExpr, TypedExprID},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprCtx {
    func: Interned<Function>,
}

impl ExprCtx {
    #[must_use]
    pub fn get_name_binding(&self, name_binding_id: NameBindingID) -> &NameBinding {
        self.func.get_name_binding(name_binding_id)
    }

    #[must_use]
    pub fn get_type_of_expr_id(&self, expr_id: TypedExprID) -> &Interned<Ty> {
        self.func.get_type_of_expr_id(expr_id)
    }

    #[must_use]
    pub fn get_typed_expr(&self, expr_id: TypedExprID) -> &TypedExpr {
        self.func.get_expression(expr_id)
    }
}
