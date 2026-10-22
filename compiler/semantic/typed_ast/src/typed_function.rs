use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_arena::{Arena, ID};
use rayc_hash::FxHashMap;
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_source_file::ByteIndex;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    subst::{MutSubstitutable, Subst},
    ty::{Ty, application::ClosureID},
};

use crate::{
    block::Block,
    member_access::MemberAccess,
    name_binding::{
        NameBinding, NameBindingGroup, NameBindingGroupID, NameBindingID, NameBindingMap,
    },
    statement::Statement,
    typed_closure::{ClosureParameterID, TypedClosureContext, TypedClosureParameter},
    typed_expr::{LvalueClassification, TypedExpr, TypedExprID, TypedExprMap},
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
    closures: FxHashMap<ClosureID, TypedFunctionID>,

    /// The member accesses of every function, in the order they were bound.
    member_accesses: Vec<MemberAccess>,
}

impl TypedFunctionMap {
    /// Creates the map of a def whose parameters can be referred to by name
    /// in `parameter_visibility`, its body.
    #[must_use]
    pub fn new(root_effect: Interned<Ty>, parameter_visibility: RelativeSpan) -> Self {
        let mut name_binding_map = NameBindingMap::default();
        let parameter_name_binding_group_id =
            name_binding_map.new_name_binding_group(parameter_visibility);

        let mut functions = Arena::default();
        let root = functions.insert(TypedFunction::new(
            TypedContext::Def(TypedDefContext::new(parameter_name_binding_group_id)),
            root_effect,
        ));

        Self {
            name_binding_map,
            functions,
            root,
            closures: FxHashMap::default(),
            member_accesses: Vec::new(),
        }
    }

    /// Assigns an owner-local identity to a nominal closure's function.
    #[must_use]
    pub fn register_closure(&mut self, function_id: TypedFunctionID) -> ClosureID {
        let _ = self.get_function(function_id).context().assert_as_closure_context();
        let closure_id = ClosureID::new(function_id.index());
        assert!(self.closures.insert(closure_id, function_id).is_none());
        closure_id
    }

    #[must_use]
    pub fn closure_function(&self, closure_id: ClosureID) -> Option<TypedFunctionID> {
        self.closures.get(&closure_id).copied()
    }

    /// Iterates over nominal closure identities and their local functions.
    #[must_use]
    pub fn closures(&self) -> impl ExactSizeIterator<Item = (ClosureID, TypedFunctionID)> {
        self.closures.iter().map(|(closure_id, function_id)| (*closure_id, *function_id))
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
            TypedContext::Closure(context) => Some(context.parameter_name_binding_group_id()),
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

    /// Inserts a closure whose parameters can be referred to by name in
    /// `parameter_visibility`, its body.
    #[must_use]
    pub fn insert_closure(
        &mut self,
        effect: Interned<Ty>,
        parameter_visibility: RelativeSpan,
    ) -> TypedFunctionID {
        let parameter_name_binding_group_id =
            self.name_binding_map.new_name_binding_group(parameter_visibility);

        self.functions.insert(TypedFunction::new(
            TypedContext::Closure(TypedClosureContext::new(parameter_name_binding_group_id)),
            effect,
        ))
    }

    /// Inserts the handler of `operation`, whose parameters can be referred
    /// to by name in `parameter_visibility`, its body.
    #[must_use]
    pub fn insert_operation_handler(
        &mut self,
        operation: GlobalSymbolID,
        return_type: Interned<Ty>,
        effect: Interned<Ty>,
        parameter_visibility: RelativeSpan,
    ) -> TypedFunctionID {
        let parameter_name_binding_group_id =
            self.name_binding_map.new_name_binding_group(parameter_visibility);

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
    pub fn insert_closure_parameter(
        &mut self,
        function_id: TypedFunctionID,
        parameter: TypedClosureParameter,
    ) -> ClosureParameterID {
        let function = self.functions.get_mut(function_id).expect("FunctionID should be valid");

        function.context.assert_as_closure_context_mut().insert_parameter(parameter)
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

    /// Iterates over all name bindings belonging to this def and their IDs.
    ///
    /// The iteration order is not stable.
    #[must_use]
    pub fn name_bindings(&self) -> impl ExactSizeIterator<Item = (NameBindingID, &NameBinding)> {
        self.name_binding_map.name_bindings()
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

    /// Creates an empty name binding group whose bindings can be referred to
    /// by name in `visibility`.
    pub fn new_name_binding_group(&mut self, visibility: RelativeSpan) -> NameBindingGroupID {
        self.name_binding_map.new_name_binding_group(visibility)
    }

    /// Iterates over the name bindings of this def that can be referred to by
    /// name at `offset`, a byte index in the source file of the def.
    ///
    /// See [`NameBindingMap::visible_name_bindings`].
    pub async fn visible_name_bindings(
        &self,
        engine: &TrackedEngine,
        offset: ByteIndex,
    ) -> impl Iterator<Item = (NameBindingID, &NameBinding)> {
        self.name_binding_map.visible_name_bindings(engine, offset).await
    }

    /// Records a member access of this def.
    pub fn insert_member_access(&mut self, member_access: MemberAccess) {
        self.member_accesses.push(member_access);
    }

    /// Returns the type of the receiver of the member access whose member the
    /// cursor at `offset` is on, a byte index in the source file of the def.
    ///
    /// The accesses of a chain such as `p.inner.x` do not overlap, so at most
    /// one access holds the cursor.
    pub async fn receiver_type_at(
        &self,
        engine: &TrackedEngine,
        offset: ByteIndex,
    ) -> Option<&Interned<Ty>> {
        for member_access in &self.member_accesses {
            if !member_access.holds_cursor(engine, offset).await {
                continue;
            }

            let receiver = member_access.receiver();
            return Some(
                self.get_function(receiver.function_id()).get_type_of_expr_id(receiver.local_id()),
            );
        }

        None
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

    /// Iterates over all expressions of this function and their IDs.
    ///
    /// The iteration order is not stable.
    #[must_use]
    pub fn expressions(&self) -> impl ExactSizeIterator<Item = (TypedExprID, &TypedExpr)> {
        self.typed_expr_map.expressions()
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
            TypedContext::Closure(context) => context.apply_mut_subst(subst, engine),
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
    Closure(TypedClosureContext),
    OperationHandler(TypedOperationHandlerContext),
    Thunk(TypedThunkContext),
}

impl TypedContext {
    #[must_use]
    #[track_caller]
    pub fn assert_as_def_context(&self) -> &TypedDefContext {
        match self {
            Self::Def(context) => context,
            Self::Closure(_) | Self::OperationHandler(_) | Self::Thunk(_) => {
                panic!("expected a def context, found a nested function context")
            }
        }
    }

    #[must_use]
    #[track_caller]
    pub fn assert_as_closure_context(&self) -> &TypedClosureContext {
        match self {
            Self::Def(_) | Self::OperationHandler(_) | Self::Thunk(_) => {
                panic!("expected a closure context, found a non-closure context")
            }
            Self::Closure(context) => context,
        }
    }

    #[track_caller]
    fn assert_as_closure_context_mut(&mut self) -> &mut TypedClosureContext {
        match self {
            Self::Def(_) | Self::OperationHandler(_) | Self::Thunk(_) => {
                panic!("expected a closure context, found a non-closure context")
            }
            Self::Closure(context) => context,
        }
    }

    #[must_use]
    #[track_caller]
    pub fn assert_as_operation_handler_context(&self) -> &TypedOperationHandlerContext {
        match self {
            Self::Def(_) | Self::Closure(_) | Self::Thunk(_) => {
                panic!("expected an operation handler context, found another function context")
            }
            Self::OperationHandler(context) => context,
        }
    }

    #[track_caller]
    fn assert_as_operation_handler_context_mut(&mut self) -> &mut TypedOperationHandlerContext {
        match self {
            Self::Def(_) | Self::Closure(_) | Self::Thunk(_) => {
                panic!("expected an operation handler context, found another function context")
            }
            Self::OperationHandler(context) => context,
        }
    }

    #[must_use]
    #[track_caller]
    pub fn assert_as_thunk_context(&self) -> &TypedThunkContext {
        match self {
            Self::Def(_) | Self::Closure(_) | Self::OperationHandler(_) => {
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
