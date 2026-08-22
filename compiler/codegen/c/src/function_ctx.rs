use qbice::storage::intern::Interned;
use rayc_type::ty::Ty;
use rayc_typed_ast::{
    function::Function,
    name_binding::{NameBinding, NameBindingID},
    statement::Statement,
    typed_expr::{TypedExpr, TypedExprID},
    variable::{Variable, VariableID},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionCtx {
    func: Interned<Function>,
}

impl FunctionCtx {
    #[must_use]
    pub const fn new(func: Interned<Function>) -> Self { Self { func } }

    pub fn statements(&self) -> impl Iterator<Item = &Statement> { self.func.statements() }

    #[must_use]
    pub fn get_variable(&self, variable_id: VariableID) -> &Variable {
        self.func.get_variable(variable_id)
    }

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
