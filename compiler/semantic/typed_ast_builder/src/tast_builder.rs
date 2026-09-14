use qbice::storage::intern::Interned;
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::{
    effect_row::get_effect_row,
    parameter::{ParameterMap, get_parameter_map},
    return_type::get_return_type,
};
use rayc_symbol::{
    GlobalSymbolID,
    syntax::{get_def_body_syntax, get_parameter_list_syntax},
};
use rayc_syntax::def::ParameterList;
use rayc_type::{
    subst::MutSubstitutable,
    ty::{InferenceConstraint, Ty, TyKind, application::ClosureID, inference::GenInfer},
};
use rayc_typed_ast::{
    TypedAst,
    capture_plan::CapturePlan,
    name_binding::{NameBindingGroupID, NameBindingID},
    statement::Statement,
    typed_expr::{self, SubExprs, TypedExpr, TypedExprID, TypedExprKind},
    typed_function::{TypedContext, TypedFunctionID, TypedFunctionLocalID, TypedFunctionMap},
    typed_lambda::{LambdaParameterID, TypedLambdaParameter},
    typed_operation_handler::{OperationHandlerParameterID, TypedOperationHandlerParameter},
    typed_variable::{TypedVariable, TypedVariableID},
};

use crate::{
    diagnostic::Diagnostic,
    tast_builder::{
        constraint_solver::ConstraintSolver, lvalue_requirements::LvalueRequirements,
        name_env::NameEnv,
    },
};

pub mod constraint_solver;
pub mod lvalue_requirements;
pub mod name_env;
pub mod resolution;

#[derive(Debug)]
pub struct TAstBuilder {
    function_map: TypedFunctionMap,
    building_function: TypedFunctionID,
    suspended_functions: Vec<TypedFunctionID>,

    statement_blocks: Vec<(TypedFunctionID, Vec<Statement>)>,
    loop_depth: usize,
    suspended_loop_depths: Vec<usize>,

    closure_captures: Vec<(TypedFunctionID, Interned<Ty>, RelativeSpan)>,
    current_def_id: GlobalSymbolID,

    name_env: NameEnv,

    constraint_solver: ConstraintSolver,
    lvalue_requirements: LvalueRequirements,

    diagnostics: Vec<Diagnostic>,
    engine: TrackedEngine,
}

impl TAstBuilder {
    pub(crate) const fn current_def_id(&self) -> GlobalSymbolID { self.current_def_id }

    pub async fn new(engine: TrackedEngine, current_def_id: GlobalSymbolID) -> Self {
        let mut constraint_solver = ConstraintSolver::new(engine.clone(), current_def_id).await;
        let root_effect = engine.intern(Ty::Inference(
            constraint_solver.gen_infer(TyKind::EffectRow, InferenceConstraint::Any),
        ));
        let function_map = TypedFunctionMap::new(root_effect);
        let building_function = function_map.root_id();
        let name_env = NameEnv::new(function_map.parameter_name_binding_group_id_of_root());

        Self {
            function_map,
            building_function,
            suspended_functions: Vec::new(),
            statement_blocks: Vec::new(),
            loop_depth: 0,
            suspended_loop_depths: Vec::new(),
            closure_captures: Vec::new(),
            name_env,
            current_def_id,
            constraint_solver,
            lvalue_requirements: LvalueRequirements::new(),
            diagnostics: Vec::new(),
            engine,
        }
    }
}

impl TAstBuilder {
    #[must_use]
    pub fn type_of_expression(&self, id: TypedExprID) -> Interned<Ty> {
        self.function_map.get_expression(self.building_function, id).ty().clone()
    }

    #[must_use]
    pub fn effect_of_expression(&self, id: TypedExprID) -> &Interned<Ty> {
        self.function_map.get_expression(self.building_function, id).effect()
    }

    #[must_use]
    pub fn get_expression(&self, id: TypedExprID) -> &TypedExpr {
        self.function_map.get_expression(self.building_function, id)
    }

    #[must_use]
    pub fn type_of_local_expression(&self, id: TypedFunctionLocalID<TypedExprID>) -> Interned<Ty> {
        self.function_map.get_expression(id.function_id(), id.local_id()).ty().clone()
    }

    #[must_use]
    pub fn type_of_name_binding(&self, name_binding_id: NameBindingID) -> Interned<Ty> {
        self.function_map.get_name_binding(name_binding_id).ty().clone()
    }

    #[must_use]
    pub fn insert_variable(&mut self, variable: TypedVariable) -> TypedVariableID {
        self.function_map.insert_variable(self.building_function, variable)
    }

    #[must_use]
    pub const fn current_typed_function_id(&self) -> TypedFunctionID { self.building_function }

    #[must_use]
    pub fn parameter_name_binding_group(&self) -> NameBindingGroupID {
        self.function_map
            .parameter_name_binding_group_id_of(self.building_function)
            .expect("the current function should have a parameter name-binding group")
    }

    #[must_use]
    pub fn start_lambda(&mut self) -> TypedFunctionID {
        let effect = self.new_effect_inference();
        let function_id = self.function_map.insert_lambda(effect);
        let parameter_name_binding_group_id =
            self.function_map.parameter_name_binding_group_id_of(function_id);

        self.suspended_functions.push(self.building_function);
        self.suspended_loop_depths.push(self.loop_depth);
        self.building_function = function_id;
        self.loop_depth = 0;
        self.name_env.enter_function(parameter_name_binding_group_id);

        function_id
    }

    pub(crate) fn defer_closure_captures(
        &mut self,
        function_id: TypedFunctionID,
        span: RelativeSpan,
    ) -> Interned<Ty> {
        let inference = self.new_type_inference();
        self.closure_captures.push((function_id, inference.clone(), span));
        inference
    }

    pub(crate) fn register_closure(&mut self, function_id: TypedFunctionID) -> ClosureID {
        self.function_map.register_closure(function_id)
    }

    #[must_use]
    pub fn finish_lambda(&mut self) -> Interned<Ty> {
        let effect = self.function_map.effect_of(self.building_function).clone();
        self.name_env.exit_function();
        self.loop_depth = self.suspended_loop_depths.pop().unwrap();
        self.building_function =
            self.suspended_functions.pop().expect("a lambda should suspend its enclosing function");
        effect
    }

    #[must_use]
    pub fn start_operation_handler(
        &mut self,
        operation: GlobalSymbolID,
        return_type: Interned<Ty>,
    ) -> TypedFunctionID {
        let effect = self.new_effect_inference();
        let function_id =
            self.function_map.insert_operation_handler(operation, return_type, effect);
        let parameter_name_binding_group_id =
            self.function_map.parameter_name_binding_group_id_of(function_id);

        self.suspended_functions.push(self.building_function);
        self.suspended_loop_depths.push(self.loop_depth);
        self.building_function = function_id;
        self.loop_depth = 0;
        self.name_env.enter_function(parameter_name_binding_group_id);

        function_id
    }

    pub fn finish_operation_handler(&mut self) {
        self.name_env.exit_function();
        self.loop_depth = self.suspended_loop_depths.pop().unwrap();
        self.building_function = self
            .suspended_functions
            .pop()
            .expect("an operation handler should suspend its enclosing function");
    }

    #[must_use]
    pub fn start_thunk(&mut self) -> (TypedFunctionID, Interned<Ty>) {
        let return_type = self.new_type_inference();
        let effect = self.new_effect_inference();
        let function_id = self.function_map.insert_thunk(return_type.clone(), effect);

        self.suspended_functions.push(self.building_function);
        self.suspended_loop_depths.push(self.loop_depth);
        self.building_function = function_id;
        self.loop_depth = 0;
        self.name_env.enter_function(None);

        (function_id, return_type)
    }

    pub fn finish_thunk(&mut self) {
        self.name_env.exit_function();
        self.loop_depth = self.suspended_loop_depths.pop().unwrap();
        self.building_function =
            self.suspended_functions.pop().expect("a thunk should suspend its enclosing function");
    }

    #[must_use]
    pub fn insert_lambda_parameter(
        &mut self,
        parameter: TypedLambdaParameter,
    ) -> LambdaParameterID {
        self.function_map.insert_lambda_parameter(self.building_function, parameter)
    }

    #[must_use]
    pub fn insert_operation_handler_parameter(
        &mut self,
        parameter: TypedOperationHandlerParameter,
    ) -> OperationHandlerParameterID {
        self.function_map.insert_operation_handler_parameter(self.building_function, parameter)
    }

    #[must_use]
    pub const fn engine(&self) -> &TrackedEngine { &self.engine }

    /// Inserts an expression into the typed AST and returns its ID.
    ///
    /// This function only requires the kind, span, and type of the expression.
    /// The effect is automatically composed from its sub-expressions.
    #[must_use]
    pub async fn insert_expression<K: Into<TypedExprKind> + SubExprs>(
        &mut self,
        kind: K,
        span: RelativeSpan,
        ty: Interned<Ty>,
    ) -> TypedExprID {
        let effect = self.new_effect_inference();
        let expr = TypedExpr::new(kind.into(), span, ty, effect);
        let id = self.function_map.insert_expression(self.building_function, expr);

        // automatically compose the effect of the expression from its sub-expressions
        self.compose_effect_from_sub_exprs(id).await;

        id
    }

    /// Inserts an expression whose effect requires custom composition rules.
    #[must_use]
    pub(super) fn insert_expression_without_effect_composition<K: Into<TypedExprKind>>(
        &mut self,
        kind: K,
        span: RelativeSpan,
        ty: Interned<Ty>,
    ) -> TypedExprID {
        let effect = self.new_effect_inference();
        let expr = TypedExpr::new(kind.into(), span, ty, effect);

        self.function_map.insert_expression(self.building_function, expr)
    }

    pub fn push_diagnostic(&mut self, diagnostic: Diagnostic) { self.diagnostics.push(diagnostic); }

    pub fn extend_diagnostics(&mut self, diagnostics: impl IntoIterator<Item = Diagnostic>) {
        self.diagnostics.extend(diagnostics);
    }

    pub async fn push_error_expression(&mut self, span: RelativeSpan) -> TypedExprID {
        let infer = self.new_type_inference();
        self.insert_expression(typed_expr::errored::Errored::new_empty(), span, infer).await
    }

    pub async fn push_error_expression_with_children(
        &mut self,
        span: RelativeSpan,
        children: Vec<typed_expr::errored::ErroredChild>,
    ) -> TypedExprID {
        let infer = self.new_type_inference();
        self.insert_expression(typed_expr::errored::Errored::new(children), span, infer).await
    }

    pub async fn push_error_expression_with_expression_children(
        &mut self,
        span: RelativeSpan,
        children: Vec<TypedExprID>,
    ) -> TypedExprID {
        self.push_error_expression_with_children(
            span,
            children.into_iter().map(Into::into).collect(),
        )
        .await
    }

    pub fn span_of_expression(&self, id: TypedExprID) -> RelativeSpan {
        self.function_map.get_expression(self.building_function, id).span()
    }

    pub fn span_of_local_expression(&self, id: TypedFunctionLocalID<TypedExprID>) -> RelativeSpan {
        self.function_map.get_expression(id.function_id(), id.local_id()).span()
    }

    pub async fn push_statement(&mut self, statement: Statement) {
        // Nested statements contribute through their owning control-flow
        // expression. Only function-root statements compose directly into the
        // function effect.
        let is_nested =
            self.statement_blocks.last().is_some_and(|(owner, _)| *owner == self.building_function);

        if is_nested {
            self.statement_blocks
                .last_mut()
                .expect("a nested statement block should be active")
                .1
                .push(statement);
            return;
        }

        self.compose_function_effect_from_statement(&statement).await;
        self.function_map.push_statement(self.building_function, statement);
    }

    pub fn enter_statement_block(&mut self, is_loop_body: bool) {
        self.loop_depth += usize::from(is_loop_body);
        self.name_env.enter_scope();
        self.statement_blocks.push((self.building_function, Vec::new()));
    }

    #[must_use]
    pub fn exit_statement_block(&mut self, is_loop_body: bool) -> Vec<Statement> {
        let (owner, statements) =
            self.statement_blocks.pop().expect("a statement block should be active");
        assert_eq!(owner, self.building_function);
        self.name_env.exit_scope();
        self.loop_depth -= usize::from(is_loop_body);
        statements
    }

    #[must_use]
    pub const fn is_inside_loop(&self) -> bool { self.loop_depth > 0 }

    #[must_use]
    pub async fn return_type_of_current_function(&self) -> Interned<Ty> {
        match self.function_map.get_function(self.building_function).context() {
            TypedContext::Def(_) => self.engine.get_return_type(self.current_def_id).await,
            TypedContext::Lambda(_) => {
                panic!("a lambda expression body should not bind return statements")
            }
            TypedContext::OperationHandler(context) => context.return_type().clone(),
            TypedContext::Thunk(context) => context.return_type().clone(),
        }
    }

    #[must_use]
    pub(crate) async fn effect_row_of_current_function(&self) -> Interned<Ty> {
        self.engine.get_effect_row(self.current_def_id).await
    }

    #[must_use]
    pub async fn parameter_map_of_current_function(&self) -> Interned<ParameterMap> {
        self.engine.get_parameter_map(self.current_def_id).await
    }

    #[must_use]
    pub async fn parameter_list_syntax_of_current_function(&self) -> Option<ParameterList> {
        self.engine.get_parameter_list_syntax(self.current_def_id).await
    }

    #[must_use]
    pub async fn def_body_syntax_of_current_function(
        &self,
    ) -> Option<rayc_syntax::statement::Block> {
        self.engine.get_def_body_syntax(self.current_def_id).await
    }
}

impl TAstBuilder {
    #[must_use]
    pub async fn finish(mut self) -> (TypedAst, Vec<Diagnostic>) {
        assert!(
            self.suspended_functions.is_empty(),
            "all suspended functions should be restored before finishing the typed AST"
        );

        // Capture layouts are structural, so their binding types may still contain
        // inference variables. Each binding wakes constraints waiting on a closure.
        let captures = CapturePlan::analyze(&self.function_map);
        for (function_id, inference, span) in std::mem::take(&mut self.closure_captures) {
            let tuple = captures.plan(function_id).captured_tuple(&self.engine);
            self.push_capture_constraint(&inference, &tuple, span).await;
        }

        self.validate_lvalue_requirements().await;
        self.finish_constraints().await;

        let (constr_diags, subst) = self.constraint_solver.residual_into_diags().await;

        self.diagnostics.extend(constr_diags);
        let mut ast = TypedAst::new(self.function_map, captures);
        ast.apply_mut_subst(&subst, &self.engine);

        (ast, self.diagnostics)
    }
}
