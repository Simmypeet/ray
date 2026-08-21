use qbice::storage::intern::Interned;
use rayc_typed_ast::{
    function::Function,
    name_binding::{NameBinding, NameBindingID},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprCtx {
    func: Interned<Function>,
}

impl ExprCtx {
    pub fn get_name_binding(&self, name_binding_id: NameBindingID) -> &NameBinding {
        self.func.get_name_binding(name_binding_id)
    }
}
