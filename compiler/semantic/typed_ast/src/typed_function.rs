use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_arena::{Arena, ID};
use rayc_qbice::TrackedEngine;
use rayc_type::{
    subst::{MutSubstitutable, Subst},
    ty::Ty,
};

use crate::{
    block::Block,
    lambda::{LambdaContext, LambdaParameter, LambdaParameterID},
    name_binding::{
        NameBinding, NameBindingGroup, NameBindingGroupID, NameBindingID, NameBindingMap,
    },
    statement::Statement,
    typed_expr::{LvalueClassification, TypedExpr, TypedExprID, TypedExprMap},
    variable::{Variable, VariableID, VariableMap},
};

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct TypedFunctionMap {
    name_binding_map: NameBindingMap,
    functions: Arena<TypedFunction>,
    root: FunctionID,
}

impl Default for TypedFunctionMap {
    fn default() -> Self {
        let mut name_binding_map = NameBindingMap::default();
        let parameter_name_binding_group_id = name_binding_map.new_name_binding_group();

        let mut functions = Arena::default();
        let root = functions.insert(TypedFunction::new(Context::Def(DefContext::new(
            parameter_name_binding_group_id,
        ))));

        Self { name_binding_map, functions, root }
    }
}

impl TypedFunctionMap {
    #[must_use]
    pub const fn root_id(&self) -> FunctionID { self.root }

    #[must_use]
    pub fn root(&self) -> &TypedFunction {
        self.functions.get(self.root).expect("Root function should exist")
    }

    #[must_use]
    pub fn get_function(&self, id: FunctionID) -> &TypedFunction {
        self.functions.get(id).expect("FunctionID should be valid")
    }

    #[must_use]
    pub fn get_expression_in(&self, function_id: FunctionID, id: TypedExprID) -> &TypedExpr {
        self.get_function(function_id).get_expression(id)
    }

    pub fn statements_in(&self, function_id: FunctionID) -> impl Iterator<Item = &Statement> {
        self.get_function(function_id).statements()
    }

    #[must_use]
    pub fn classify_lvalue_in(
        &self,
        function_id: FunctionID,
        id: TypedExprID,
    ) -> LvalueClassification {
        self.get_function(function_id).classify_lvalue(id)
    }

    #[must_use]
    pub fn parameter_name_binding_group_id_of(
        &self,
        function_id: FunctionID,
    ) -> NameBindingGroupID {
        match self.get_function(function_id).context() {
            Context::Def(context) => context.parameter_name_binding_group_id(),
            Context::Lambda(context) => context.parameter_name_binding_group_id(),
        }
    }

    #[must_use]
    pub fn parameter_name_binding_group_id_of_root(&self) -> NameBindingGroupID {
        self.parameter_name_binding_group_id_of(self.root)
    }

    /// Iterates over all functions belonging to this def and their IDs.
    ///
    /// The iteration order is not stable.
    #[must_use]
    pub fn functions(&self) -> impl ExactSizeIterator<Item = (FunctionID, &TypedFunction)> {
        self.functions.iter()
    }

    #[must_use]
    pub fn insert_lambda(&mut self) -> FunctionID {
        let parameter_name_binding_group_id = self.name_binding_map.new_name_binding_group();

        self.functions.insert(TypedFunction::new(Context::Lambda(LambdaContext::new(
            parameter_name_binding_group_id,
        ))))
    }

    #[must_use]
    pub fn insert_lambda_parameter(
        &mut self,
        function_id: FunctionID,
        parameter: LambdaParameter,
    ) -> LambdaParameterID {
        let function = self.functions.get_mut(function_id).expect("FunctionID should be valid");

        match &mut function.context {
            Context::Def(_) => panic!("lambda parameters cannot be inserted into a def"),
            Context::Lambda(context) => context.insert_parameter(parameter),
        }
    }

    #[must_use]
    pub fn insert_expression_into(
        &mut self,
        function_id: FunctionID,
        expression: TypedExpr,
    ) -> TypedExprID {
        self.functions
            .get_mut(function_id)
            .expect("FunctionID should be valid")
            .insert_expression(expression)
    }

    #[must_use]
    pub fn insert_variable_into(
        &mut self,
        function_id: FunctionID,
        variable: Variable,
    ) -> VariableID {
        self.functions
            .get_mut(function_id)
            .expect("FunctionID should be valid")
            .insert_variable(variable)
    }

    pub fn push_statement_into(&mut self, function_id: FunctionID, statement: Statement) {
        self.functions
            .get_mut(function_id)
            .expect("FunctionID should be valid")
            .push_statement(statement);
    }

    #[must_use]
    pub fn get_name_binding(&self, id: NameBindingID) -> &NameBinding {
        self.name_binding_map.get_name_binding(id)
    }

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
}

impl MutSubstitutable for TypedFunctionMap {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        self.name_binding_map.apply_mut_subst(subst, engine);

        for function in self.functions.items_mut() {
            function.apply_mut_subst(subst, engine);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct TypedFunction {
    variable_map: VariableMap,
    typed_expr_map: TypedExprMap,
    block: Block,
    context: Context,
}

impl TypedFunction {
    fn new(context: Context) -> Self {
        Self {
            variable_map: VariableMap::default(),
            typed_expr_map: TypedExprMap::default(),
            block: Block::default(),
            context,
        }
    }

    #[must_use]
    pub const fn context(&self) -> &Context { &self.context }

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
    pub fn get_variable(&self, id: VariableID) -> &Variable { self.variable_map.get_variable(id) }

    #[must_use]
    fn insert_expression(&mut self, expression: TypedExpr) -> TypedExprID {
        self.typed_expr_map.insert_expression(expression)
    }

    #[must_use]
    fn insert_variable(&mut self, variable: Variable) -> VariableID {
        self.variable_map.insert_variable(variable)
    }

    fn push_statement(&mut self, statement: Statement) { self.block.push_statement(statement); }

    #[must_use]
    pub fn get_type_of_expr_id(&self, expr_id: TypedExprID) -> &Interned<Ty> {
        self.typed_expr_map.get_expression(expr_id).ty()
    }
}

impl MutSubstitutable for TypedFunction {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        self.variable_map.apply_mut_subst(subst, engine);
        self.typed_expr_map.apply_mut_subst(subst, engine);

        match &mut self.context {
            Context::Def(_) => {}
            Context::Lambda(context) => context.apply_mut_subst(subst, engine),
        }
    }
}

pub type FunctionID = ID<TypedFunction>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct FunctionLocalID<LocalID> {
    function_id: FunctionID,
    local_id: LocalID,
}

impl<LocalID> FunctionLocalID<LocalID> {
    #[must_use]
    pub const fn new(function_id: FunctionID, local_id: LocalID) -> Self {
        Self { function_id, local_id }
    }

    #[must_use]
    pub const fn function_id(&self) -> FunctionID { self.function_id }

    #[must_use]
    pub const fn local_id(&self) -> LocalID
    where
        LocalID: Copy,
    {
        self.local_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub enum Context {
    Def(DefContext),
    Lambda(LambdaContext),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, StableHash, Encode, Decode)]
pub struct DefContext {
    parameter_name_binding_group_id: NameBindingGroupID,
}

impl DefContext {
    const fn new(parameter_name_binding_group_id: NameBindingGroupID) -> Self {
        Self { parameter_name_binding_group_id }
    }

    #[must_use]
    pub const fn parameter_name_binding_group_id(&self) -> NameBindingGroupID {
        self.parameter_name_binding_group_id
    }
}
