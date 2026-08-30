use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_arena::{Arena, ID};
use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;
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
    typed_lambda::{LambdaParameterID, TypedLambdaContext, TypedLambdaParameter},
    typed_operation_handler::{
        OperationHandlerParameterID, TypedOperationHandlerContext, TypedOperationHandlerParameter,
    },
    typed_thunk::TypedThunkContext,
    typed_variable::{TypedVariable, TypedVariableID, TypedVariableMap},
};

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct TypedFunctionMap {
    name_binding_map: NameBindingMap,
    functions: Arena<TypedFunction>,
    root: TypedFunctionID,
}

impl TypedFunctionMap {
    #[must_use]
    pub fn new(root_effect: Interned<Ty>) -> Self {
        let mut name_binding_map = NameBindingMap::default();
        let parameter_name_binding_group_id = name_binding_map.new_name_binding_group();

        let mut functions = Arena::default();
        let root = functions.insert(TypedFunction::new(
            TypedContext::Def(TypedDefContext::new(parameter_name_binding_group_id)),
            root_effect,
        ));

        Self { name_binding_map, functions, root }
    }
    #[must_use]
    pub const fn root_id(&self) -> TypedFunctionID { self.root }

    #[must_use]
    pub fn root(&self) -> &TypedFunction {
        self.functions.get(self.root).expect("Root function should exist")
    }

    #[must_use]
    pub fn get_function(&self, id: TypedFunctionID) -> &TypedFunction {
        self.functions.get(id).expect("FunctionID should be valid")
    }

    #[must_use]
    pub fn get_expression(&self, function_id: TypedFunctionID, id: TypedExprID) -> &TypedExpr {
        self.get_function(function_id).get_expression(id)
    }

    pub fn statements(&self, function_id: TypedFunctionID) -> impl Iterator<Item = &Statement> {
        self.get_function(function_id).statements()
    }

    #[must_use]
    pub fn effect_of(&self, function_id: TypedFunctionID) -> &Interned<Ty> {
        self.get_function(function_id).effect()
    }

    #[must_use]
    pub fn classify_lvalue(
        &self,
        function_id: TypedFunctionID,
        id: TypedExprID,
    ) -> LvalueClassification {
        self.get_function(function_id).classify_lvalue(id)
    }

    #[must_use]
    pub fn parameter_name_binding_group_id_of(
        &self,
        function_id: TypedFunctionID,
    ) -> Option<NameBindingGroupID> {
        match self.get_function(function_id).context() {
            TypedContext::Def(context) => Some(context.parameter_name_binding_group_id()),
            TypedContext::Lambda(context) => Some(context.parameter_name_binding_group_id()),
            TypedContext::OperationHandler(context) => {
                Some(context.parameter_name_binding_group_id())
            }
            TypedContext::Thunk(_) => None,
        }
    }

    #[must_use]
    pub fn parameter_name_binding_group_id_of_root(&self) -> NameBindingGroupID {
        self.parameter_name_binding_group_id_of(self.root)
            .expect("the root function should have a parameter name-binding group")
    }

    /// Iterates over all functions belonging to this def and their IDs.
    ///
    /// The iteration order is not stable.
    #[must_use]
    pub fn functions(&self) -> impl ExactSizeIterator<Item = (TypedFunctionID, &TypedFunction)> {
        self.functions.iter()
    }

    #[must_use]
    pub fn insert_lambda(&mut self, effect: Interned<Ty>) -> TypedFunctionID {
        let parameter_name_binding_group_id = self.name_binding_map.new_name_binding_group();

        self.functions.insert(TypedFunction::new(
            TypedContext::Lambda(TypedLambdaContext::new(parameter_name_binding_group_id)),
            effect,
        ))
    }

    #[must_use]
    pub fn insert_operation_handler(
        &mut self,
        operation: GlobalSymbolID,
        return_type: Interned<Ty>,
        effect: Interned<Ty>,
    ) -> TypedFunctionID {
        let parameter_name_binding_group_id = self.name_binding_map.new_name_binding_group();

        self.functions.insert(TypedFunction::new(
            TypedContext::OperationHandler(TypedOperationHandlerContext::new(
                operation,
                parameter_name_binding_group_id,
                return_type,
            )),
            effect,
        ))
    }

    #[must_use]
    pub fn insert_thunk(
        &mut self,
        return_type: Interned<Ty>,
        effect: Interned<Ty>,
    ) -> TypedFunctionID {
        self.functions.insert(TypedFunction::new(
            TypedContext::Thunk(TypedThunkContext::new(return_type)),
            effect,
        ))
    }

    #[must_use]
    pub fn insert_lambda_parameter(
        &mut self,
        function_id: TypedFunctionID,
        parameter: TypedLambdaParameter,
    ) -> LambdaParameterID {
        let function = self.functions.get_mut(function_id).expect("FunctionID should be valid");

        function.context.assert_as_lambda_context_mut().insert_parameter(parameter)
    }

    #[must_use]
    pub fn insert_operation_handler_parameter(
        &mut self,
        function_id: TypedFunctionID,
        parameter: TypedOperationHandlerParameter,
    ) -> OperationHandlerParameterID {
        let function = self.functions.get_mut(function_id).expect("FunctionID should be valid");

        function.context.assert_as_operation_handler_context_mut().insert_parameter(parameter)
    }

    #[must_use]
    pub fn insert_expression(
        &mut self,
        function_id: TypedFunctionID,
        expression: TypedExpr,
    ) -> TypedExprID {
        self.functions
            .get_mut(function_id)
            .expect("FunctionID should be valid")
            .insert_expression(expression)
    }

    #[must_use]
    pub fn insert_variable(
        &mut self,
        function_id: TypedFunctionID,
        variable: TypedVariable,
    ) -> TypedVariableID {
        self.functions
            .get_mut(function_id)
            .expect("FunctionID should be valid")
            .insert_variable(variable)
    }

    pub fn push_statement(&mut self, function_id: TypedFunctionID, statement: Statement) {
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
    variable_map: TypedVariableMap,
    typed_expr_map: TypedExprMap,
    block: Block,
    context: TypedContext,
}

impl TypedFunction {
    fn new(context: TypedContext, effect: Interned<Ty>) -> Self {
        Self {
            variable_map: TypedVariableMap::default(),
            typed_expr_map: TypedExprMap::default(),
            block: Block::new(effect),
            context,
        }
    }

    #[must_use]
    pub const fn context(&self) -> &TypedContext { &self.context }

    pub fn statements(&self) -> impl Iterator<Item = &Statement> { self.block.statements() }

    #[must_use]
    const fn effect(&self) -> &Interned<Ty> { self.block.effect() }

    #[must_use]
    pub fn get_expression(&self, id: TypedExprID) -> &TypedExpr {
        self.typed_expr_map.get_expression(id)
    }

    #[must_use]
    pub fn classify_lvalue(&self, id: TypedExprID) -> LvalueClassification {
        self.typed_expr_map.classify_lvalue(id)
    }

    #[must_use]
    pub fn get_variable(&self, id: TypedVariableID) -> &TypedVariable {
        self.variable_map.get_variable(id)
    }

    #[must_use]
    fn insert_expression(&mut self, expression: TypedExpr) -> TypedExprID {
        self.typed_expr_map.insert_expression(expression)
    }

    #[must_use]
    fn insert_variable(&mut self, variable: TypedVariable) -> TypedVariableID {
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
        self.block.apply_mut_subst(subst, engine);

        match &mut self.context {
            TypedContext::Def(_) => {}
            TypedContext::Lambda(context) => context.apply_mut_subst(subst, engine),
            TypedContext::OperationHandler(context) => context.apply_mut_subst(subst, engine),
            TypedContext::Thunk(context) => context.apply_mut_subst(subst, engine),
        }
    }
}

pub type TypedFunctionID = ID<TypedFunction>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct TypedFunctionLocalID<LocalID> {
    function_id: TypedFunctionID,
    local_id: LocalID,
}

impl<LocalID> TypedFunctionLocalID<LocalID> {
    #[must_use]
    pub const fn new(function_id: TypedFunctionID, local_id: LocalID) -> Self {
        Self { function_id, local_id }
    }

    #[must_use]
    pub const fn function_id(&self) -> TypedFunctionID { self.function_id }

    #[must_use]
    pub const fn local_id(&self) -> LocalID
    where
        LocalID: Copy,
    {
        self.local_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub enum TypedContext {
    Def(TypedDefContext),
    Lambda(TypedLambdaContext),
    OperationHandler(TypedOperationHandlerContext),
    Thunk(TypedThunkContext),
}

impl TypedContext {
    #[must_use]
    #[track_caller]
    pub fn assert_as_def_context(&self) -> &TypedDefContext {
        match self {
            Self::Def(context) => context,
            Self::Lambda(_) | Self::OperationHandler(_) | Self::Thunk(_) => {
                panic!("expected a def context, found a nested function context")
            }
        }
    }

    #[must_use]
    #[track_caller]
    pub fn assert_as_lambda_context(&self) -> &TypedLambdaContext {
        match self {
            Self::Def(_) | Self::OperationHandler(_) | Self::Thunk(_) => {
                panic!("expected a lambda context, found a non-lambda context")
            }
            Self::Lambda(context) => context,
        }
    }

    #[track_caller]
    fn assert_as_lambda_context_mut(&mut self) -> &mut TypedLambdaContext {
        match self {
            Self::Def(_) | Self::OperationHandler(_) | Self::Thunk(_) => {
                panic!("expected a lambda context, found a non-lambda context")
            }
            Self::Lambda(context) => context,
        }
    }

    #[must_use]
    #[track_caller]
    pub fn assert_as_operation_handler_context(&self) -> &TypedOperationHandlerContext {
        match self {
            Self::Def(_) | Self::Lambda(_) | Self::Thunk(_) => {
                panic!("expected an operation handler context, found another function context")
            }
            Self::OperationHandler(context) => context,
        }
    }

    #[track_caller]
    fn assert_as_operation_handler_context_mut(&mut self) -> &mut TypedOperationHandlerContext {
        match self {
            Self::Def(_) | Self::Lambda(_) | Self::Thunk(_) => {
                panic!("expected an operation handler context, found another function context")
            }
            Self::OperationHandler(context) => context,
        }
    }

    #[must_use]
    #[track_caller]
    pub fn assert_as_thunk_context(&self) -> &TypedThunkContext {
        match self {
            Self::Def(_) | Self::Lambda(_) | Self::OperationHandler(_) => {
                panic!("expected a thunk context, found another function context")
            }
            Self::Thunk(context) => context,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, StableHash, Encode, Decode)]
pub struct TypedDefContext {
    parameter_name_binding_group_id: NameBindingGroupID,
}

impl TypedDefContext {
    const fn new(parameter_name_binding_group_id: NameBindingGroupID) -> Self {
        Self { parameter_name_binding_group_id }
    }

    #[must_use]
    pub const fn parameter_name_binding_group_id(&self) -> NameBindingGroupID {
        self.parameter_name_binding_group_id
    }
}
