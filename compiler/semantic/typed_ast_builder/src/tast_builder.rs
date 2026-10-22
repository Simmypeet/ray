use qbice::storage::intern::Interned;
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::parameter::{ParameterMap, get_parameter_map};
use rayc_source_file::SourceElement;
use rayc_symbol::{
    GlobalSymbolID,
    span::get_span,
    syntax::{get_def_body_syntax, get_parameter_list_syntax},
};
use rayc_syntax::def::ParameterList;
use rayc_type::{
    signature::{get_effect_row, get_return_type},
    subst::MutSubstitutable,
    ty::{InferenceConstraint, Ty, TyKind, application::ClosureID, inference::GenInfer},
};
use rayc_typed_ast::{
    TypedAst,
    capture_plan::CapturePlan,
    member_access::MemberAccess,
    name_binding::{NameBindingGroupID, NameBindingID},
    statement::Statement,
    typed_closure::{ClosureParameterID, TypedClosureParameter},
    typed_expr::{self, SubExprs, TypedExpr, TypedExprID, TypedExprKind},
    typed_function::{TypedContext, TypedFunctionID, TypedFunctionLocalID, TypedFunctionMap},
    typed_operation_handler::{OperationHandlerParameterID, TypedOperationHandlerParameter},
    typed_variable::{TypedVariable, TypedVariableID},
};

use crate::{
    diagnostic::Diagnostic,
    tast_builder::{
        constraint_solver::ConstraintSolver, lvalue_requirements::LvalueRequirements,
        name_env::NameEnv, numeric_literal_ranges::NumericLiteralRanges,
    },
};

pub mod coercion;
pub mod constraint_solver;
pub mod lvalue_requirements;
pub mod name_env;
pub mod numeric_literal_ranges;
pub mod resolution;
mod type_annotation;

/// What a statement block is the body of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatementBlockKind {
    /// The body of a loop, which `break` and `continue` apply to.
    LoopBody,

    /// Any other block, such as an arm of an `if` or a clause of a `handle`.
    Plain,
}

/// A statement block that is being bound, and the statements bound in it so
/// far.
#[derive(Debug)]
struct StatementBlock {
    /// The function the block belongs to.
    owner: TypedFunctionID,
    kind: StatementBlockKind,
    statements: Vec<Statement>,
}

#[derive(Debug)]
pub struct TAstBuilder {
    function_map: TypedFunctionMap,
    building_function: TypedFunctionID,
    suspended_functions: Vec<TypedFunctionID>,

    statement_blocks: Vec<StatementBlock>,
    loop_depth: usize,
    suspended_loop_depths: Vec<usize>,

    /// How many `unsafe` expressions enclose the expression being bound.
    /// Nested functions inherit it, as Rust closures do.
    unsafe_depth: usize,

    closure_captures: Vec<(TypedFunctionID, Interned<Ty>, RelativeSpan)>,
    current_def_id: GlobalSymbolID,

    name_env: NameEnv,

    constraint_solver: ConstraintSolver,
    lvalue_requirements: LvalueRequirements,
    numeric_literal_ranges: NumericLiteralRanges,

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

        // the parameters and the names declared at the top of the body are
        // visible in the body; the name of a def whose body is missing stands
        // in for it
        let body_span = match engine.get_def_body_syntax(current_def_id).await {
            Some(body) => body.span(),
            None => engine.get_span(current_def_id).await.expect("a def is declared with a name"),
        };

        let function_map = TypedFunctionMap::new(root_effect, body_span);
        let building_function = function_map.root_id();
        let name_env =
            NameEnv::new(function_map.parameter_name_binding_group_id_of_root(), body_span);

        Self {
            function_map,
            building_function,
            suspended_functions: Vec::new(),
            statement_blocks: Vec::new(),
            loop_depth: 0,
            suspended_loop_depths: Vec::new(),
            unsafe_depth: 0,
            closure_captures: Vec::new(),
            name_env,
            current_def_id,
            constraint_solver,
            lvalue_requirements: LvalueRequirements::new(),
            numeric_literal_ranges: NumericLiteralRanges::new(),
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

    /// Starts building a closure whose body covers `body_span`.
    #[must_use]
    pub fn start_closure(&mut self, body_span: RelativeSpan) -> TypedFunctionID {
        let effect = self.new_effect_inference();
        let function_id = self.function_map.insert_closure(effect, body_span);
        let parameter_name_binding_group_id =
            self.function_map.parameter_name_binding_group_id_of(function_id);

        self.suspended_functions.push(self.building_function);
        self.suspended_loop_depths.push(self.loop_depth);
        self.building_function = function_id;
        self.loop_depth = 0;
        self.name_env.enter_scope(body_span, parameter_name_binding_group_id);

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
    pub fn finish_closure(&mut self) -> Interned<Ty> {
        let effect = self.function_map.effect_of(self.building_function).clone();
        self.name_env.exit_scope();
        self.loop_depth = self.suspended_loop_depths.pop().unwrap();
        self.building_function = self
            .suspended_functions
            .pop()
            .expect("a closure should suspend its enclosing function");
        effect
    }

    /// Starts building the handler of `operation` whose body covers
    /// `body_span`.
    #[must_use]
    pub fn start_operation_handler(
        &mut self,
        operation: GlobalSymbolID,
        return_type: Interned<Ty>,
        body_span: RelativeSpan,
    ) -> TypedFunctionID {
        let effect = self.new_effect_inference();
        let function_id =
            self.function_map.insert_operation_handler(operation, return_type, effect, body_span);
        let parameter_name_binding_group_id =
            self.function_map.parameter_name_binding_group_id_of(function_id);

        self.suspended_functions.push(self.building_function);
        self.suspended_loop_depths.push(self.loop_depth);
        self.building_function = function_id;
        self.loop_depth = 0;
        self.name_env.enter_scope(body_span, parameter_name_binding_group_id);

        function_id
    }

    pub fn finish_operation_handler(&mut self) {
        self.name_env.exit_scope();
        self.loop_depth = self.suspended_loop_depths.pop().unwrap();
        self.building_function = self
            .suspended_functions
            .pop()
            .expect("an operation handler should suspend its enclosing function");
    }

    /// Starts building a thunk whose body covers `body_span`.
    #[must_use]
    pub fn start_thunk(&mut self, body_span: RelativeSpan) -> (TypedFunctionID, Interned<Ty>) {
        let return_type = self.new_type_inference();
        let effect = self.new_effect_inference();
        let function_id = self.function_map.insert_thunk(return_type.clone(), effect);

        self.suspended_functions.push(self.building_function);
        self.suspended_loop_depths.push(self.loop_depth);
        self.building_function = function_id;
        self.loop_depth = 0;
        self.name_env.enter_scope(body_span, None);

        (function_id, return_type)
    }

    pub fn finish_thunk(&mut self) {
        self.name_env.exit_scope();
        self.loop_depth = self.suspended_loop_depths.pop().unwrap();
        self.building_function =
            self.suspended_functions.pop().expect("a thunk should suspend its enclosing function");
    }

    #[must_use]
    pub fn insert_closure_parameter(
        &mut self,
        parameter: TypedClosureParameter,
    ) -> ClosureParameterID {
        self.function_map.insert_closure_parameter(self.building_function, parameter)
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

    /// Inserts an errored expression for syntax that the parser could not
    /// parse. Its type is the error type rather than an inference, since the
    /// parser has already reported the error.
    pub async fn push_syntax_error_expression(&mut self, span: RelativeSpan) -> TypedExprID {
        let ty = Ty::new_error(TyKind::Star, &self.engine);
        self.insert_expression(typed_expr::errored::Errored::new_empty(), span, ty).await
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
        children: impl IntoIterator<Item = TypedExprID>,
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

    /// Records the access of the member at `span`, the dot and the member
    /// after it, of `receiver`, an expression of the function being built.
    pub fn record_member_access(&mut self, span: RelativeSpan, receiver: TypedExprID) {
        self.function_map.insert_member_access(MemberAccess::new(
            span,
            TypedFunctionLocalID::new(self.building_function, receiver),
        ));
    }

    pub async fn push_statement(&mut self, statement: Statement) {
        // Nested statements contribute through their owning control-flow
        // expression. Only function-root statements compose directly into the
        // function effect.
        let is_nested =
            self.statement_blocks.last().is_some_and(|block| block.owner == self.building_function);

        if is_nested {
            self.statement_blocks
                .last_mut()
                .expect("a nested statement block should be active")
                .statements
                .push(statement);
            return;
        }

        self.compose_function_effect_from_statement(&statement).await;
        self.function_map.push_statement(self.building_function, statement);
    }

    /// Opens a statement block covering `span`, the scope of the names
    /// declared in it.
    pub fn enter_statement_block(&mut self, kind: StatementBlockKind, span: RelativeSpan) {
        self.loop_depth += usize::from(kind == StatementBlockKind::LoopBody);
        self.name_env.enter_scope(span, None);
        self.statement_blocks.push(StatementBlock {
            owner: self.building_function,
            kind,
            statements: Vec::new(),
        });
    }

    /// Closes the innermost statement block and returns its statements.
    #[must_use]
    pub fn exit_statement_block(&mut self) -> Vec<Statement> {
        let StatementBlock { owner, kind, statements } =
            self.statement_blocks.pop().expect("a statement block should be active");
        assert_eq!(owner, self.building_function);
        self.name_env.exit_scope();
        self.loop_depth -= usize::from(kind == StatementBlockKind::LoopBody);
        statements
    }

    #[must_use]
    pub const fn is_inside_loop(&self) -> bool { self.loop_depth > 0 }

    pub const fn enter_unsafe(&mut self) { self.unsafe_depth += 1; }

    pub const fn exit_unsafe(&mut self) { self.unsafe_depth -= 1; }

    #[must_use]
    pub const fn is_inside_unsafe(&self) -> bool { self.unsafe_depth > 0 }

    #[must_use]
    pub async fn return_type_of_current_function(&self) -> Interned<Ty> {
        match self.function_map.get_function(self.building_function).context() {
            TypedContext::Def(_) => self.engine.get_return_type(self.current_def_id).await,
            TypedContext::Closure(_) => {
                panic!("a closure expression body should not bind return statements")
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
        let captures = CapturePlan::analyze(&self.function_map, &mut self.constraint_solver).await;
        for (function_id, inference, span) in std::mem::take(&mut self.closure_captures) {
            let tuple = captures.plan(function_id).captured_tuple(&self.engine);
            self.push_capture_constraint(&inference, &tuple, span).await;
        }

        self.validate_lvalue_requirements().await;
        self.finish_constraints().await;
        self.validate_numeric_literal_ranges().await;

        let (constr_diags, subst) = self.constraint_solver.residual_into_diags().await;

        self.diagnostics.extend(constr_diags);
        let mut ast = TypedAst::new(self.function_map, captures);
        ast.apply_mut_subst(&subst, &self.engine);

        // An undetermined type is often a consequence of another error, e.g. an
        // errored expression has a fresh type that nothing constrains. As rustc
        // does, only ask for an annotation when no other error explains it.
        if self.diagnostics.is_empty() {
            self.diagnostics.extend(
                type_annotation::type_annotation_required_diagnostics(
                    ast.functions(),
                    &self.engine,
                )
                .await,
            );
        }

        ast.set_no_error(self.diagnostics.is_empty());

        (ast, self.diagnostics)
    }
}
