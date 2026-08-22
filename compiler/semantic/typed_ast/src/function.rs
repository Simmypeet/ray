use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_qbice::TrackedEngine;
use rayc_type::{
    subst::{MutSubstitutable, Subst},
    ty::Ty,
};

use crate::{
    block::Block,
    name_binding::{
        NameBinding, NameBindingGroup, NameBindingGroupID, NameBindingID, NameBindingMap,
    },
    statement::Statement,
    typed_expr::{LvalueClassification, TypedExpr, TypedExprID, TypedExprMap},
    variable::{Variable, VariableID, VariableMap},
};

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct Function {
    variable_map: VariableMap,
    name_binding_map: NameBindingMap,
    typed_expr_map: TypedExprMap,

    parameter_name_binding_group_id: NameBindingGroupID,
    block: Block,
}

impl Default for Function {
    fn default() -> Self {
        let mut name_binding_map = NameBindingMap::default();
        let parameter_name_binding_group_id = name_binding_map.new_name_binding_group();

        Self {
            variable_map: VariableMap::default(),
            name_binding_map,
            typed_expr_map: TypedExprMap::default(),
            parameter_name_binding_group_id,
            block: Block::default(),
        }
    }
}

impl Function {
    pub fn statements(&self) -> impl Iterator<Item = &Statement> { self.block.statements() }

    #[must_use]
    pub fn get_expression(&self, id: TypedExprID) -> &TypedExpr {
        self.typed_expr_map.get_expression(id)
    }

    #[must_use]
    pub fn classify_lvalue(&self, id: TypedExprID) -> LvalueClassification {
        self.typed_expr_map.classify_lvalue(id)
    }

    #[must_use]
    pub const fn parameter_name_binding_group_id(&self) -> NameBindingGroupID {
        self.parameter_name_binding_group_id
    }

    #[must_use]
    pub fn get_variable(&self, id: VariableID) -> &Variable { self.variable_map.get_variable(id) }

    #[must_use]
    pub fn insert_name_binding(&mut self, name_binding: NameBinding) -> NameBindingID {
        self.name_binding_map.insert_name_binding(name_binding)
    }

    #[must_use]
    pub fn insert_name_binding_group(
        &mut self,
        name_binding_group: NameBindingGroup,
    ) -> NameBindingGroupID {
        self.name_binding_map.insert_name_binding_group(name_binding_group)
    }

    #[must_use]
    pub fn insert_expression(&mut self, expression: TypedExpr) -> TypedExprID {
        self.typed_expr_map.insert_expression(expression)
    }

    #[must_use]
    pub fn insert_variable(&mut self, variable: Variable) -> VariableID {
        self.variable_map.insert_variable(variable)
    }

    pub fn push_statement(&mut self, statement: Statement) { self.block.push_statement(statement); }

    #[must_use]
    pub fn lookup_name_binding(
        &self,
        group_id: NameBindingGroupID,
        name: &str,
    ) -> Option<NameBindingID> {
        self.name_binding_map.lookup_name(group_id, name)
    }

    pub fn insert_name_binding_to_group(
        &mut self,
        group_id: NameBindingGroupID,
        name_binding_id: NameBindingID,
    ) -> Result<(), NameBindingID> {
        self.name_binding_map.insert_name_binding_to_group(group_id, name_binding_id)
    }

    pub fn new_name_binding_group(&mut self) -> NameBindingGroupID {
        self.name_binding_map.new_name_binding_group()
    }

    #[must_use]
    pub fn get_name_binding(&self, id: NameBindingID) -> &NameBinding {
        self.name_binding_map.get_name_binding(id)
    }

    #[must_use]
    pub fn get_type_of_expr_id(&self, expr_id: TypedExprID) -> &Interned<Ty> {
        self.typed_expr_map.get_expression(expr_id).ty()
    }
}

impl MutSubstitutable for Function {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        self.variable_map.apply_mut_subst(subst, engine);
        self.name_binding_map.apply_mut_subst(subst, engine);
        self.typed_expr_map.apply_mut_subst(subst, engine);
    }
}
