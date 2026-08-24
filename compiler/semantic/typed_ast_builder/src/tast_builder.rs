use qbice::storage::intern::Interned;
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::{
    parameter::{ParameterMap, get_parameter_map},
    return_type::get_return_type,
};
use rayc_symbol::{
    GlobalSymbolID, get_target_root_module_id,
    member::get_members,
    syntax::{get_def_body_syntax, get_def_signature_syntax},
};
use rayc_syntax::{Identifier, def::ParameterList};
use rayc_type::{
    solver::Solver,
    subst::MutSubstitutable,
    ty::{InferenceConstraint, Ty, TyKind},
};
use rayc_typed_ast::{
    function::{FunctionID, FunctionLocalID, FunctionMap},
    lambda::{LambdaParameter, LambdaParameterID},
    name_binding::{NameBindingGroupID, NameBindingID},
    statement::Statement,
    typed_expr::{TypedExpr, TypedExprID},
    variable::{Variable, VariableID},
};

use crate::{
    diagnostic::{Diagnostic, FunctionNotFound},
    tast_builder::{
        constraint_solver::ConstraintSolver, lvalue_requirements::LvalueRequirements,
        name_env::NameEnv,
    },
};

pub mod constraint_solver;
pub mod lvalue_requirements;
pub mod name_env;

#[derive(Debug)]
pub struct TAstBuilder {
    function_map: FunctionMap,
    building_function: FunctionID,
    suspended_functions: Vec<FunctionID>,
    current_def_id: GlobalSymbolID,

    name_env: NameEnv,
    solver: Solver,

    constraint_solver: ConstraintSolver,
    lvalue_requirements: LvalueRequirements,

    diagnostics: Vec<Diagnostic>,
    engine: TrackedEngine,
}

impl TAstBuilder {
    #[must_use]
    pub fn new(engine: TrackedEngine, current_def_id: GlobalSymbolID) -> Self {
        let function_map = FunctionMap::default();
        let building_function = function_map.root_id();
        let name_env = NameEnv::new(function_map.parameter_name_binding_group_id_of_root());

        Self {
            function_map,
            building_function,
            suspended_functions: Vec::new(),
            name_env,
            current_def_id,
            solver: Solver::new(engine.clone()),
            constraint_solver: ConstraintSolver::new(),
            lvalue_requirements: LvalueRequirements::new(),
            diagnostics: Vec::new(),
            engine,
        }
    }
}

impl TAstBuilder {
    #[must_use]
    pub fn type_of_expression(&self, id: TypedExprID) -> Interned<Ty> {
        self.function_map.get_expression_in(self.building_function, id).ty().clone()
    }

    #[must_use]
    pub fn type_of_local_expression(&self, id: FunctionLocalID<TypedExprID>) -> Interned<Ty> {
        self.function_map.get_expression_in(id.function_id(), id.local_id()).ty().clone()
    }

    #[must_use]
    pub fn type_of_name_binding(&self, name_binding_id: NameBindingID) -> Interned<Ty> {
        self.function_map.get_name_binding(name_binding_id).ty().clone()
    }

    #[must_use]
    pub fn insert_variable(&mut self, variable: Variable) -> VariableID {
        self.function_map.insert_variable_into(self.building_function, variable)
    }

    #[must_use]
    pub const fn current_typed_function_id(&self) -> FunctionID { self.building_function }

    #[must_use]
    pub fn parameter_name_binding_group(&self) -> NameBindingGroupID {
        self.function_map.parameter_name_binding_group_id_of(self.building_function)
    }

    #[must_use]
    pub fn start_lambda(&mut self) -> FunctionID {
        let function_id = self.function_map.insert_lambda();
        let parameter_name_binding_group_id =
            self.function_map.parameter_name_binding_group_id_of(function_id);

        self.suspended_functions.push(self.building_function);
        self.building_function = function_id;
        self.name_env.enter_function(parameter_name_binding_group_id);

        function_id
    }

    pub fn finish_lambda(&mut self) {
        self.name_env.exit_function();
        self.building_function =
            self.suspended_functions.pop().expect("a lambda should suspend its enclosing function");
    }

    #[must_use]
    pub fn insert_lambda_parameter(&mut self, parameter: LambdaParameter) -> LambdaParameterID {
        self.function_map.insert_lambda_parameter(self.building_function, parameter)
    }

    #[must_use]
    pub const fn engine(&self) -> &TrackedEngine { &self.engine }

    /// Pushes an expression into the current block of the function being
    /// built, and returns the [`TypedExprID`] of the expression in the
    /// function's expression map.
    #[must_use]
    pub fn insert_expression(&mut self, expr: TypedExpr) -> TypedExprID {
        self.function_map.insert_expression_into(self.building_function, expr)
    }

    pub fn push_diagnostic(&mut self, diagnostic: Diagnostic) { self.diagnostics.push(diagnostic); }

    pub fn new_type_inference(&mut self) -> Interned<Ty> {
        self.new_type_inference_with_kind(TyKind::Star)
    }

    pub fn new_type_inference_with_kind(&mut self, kind: TyKind) -> Interned<Ty> {
        self.engine.intern(Ty::Inference(self.solver.new_inference(kind)))
    }

    pub fn new_numeric_type_inference(&mut self) -> Interned<Ty> {
        self.engine.intern(Ty::Inference(
            self.solver.new_inference_with_constraint(TyKind::Star, InferenceConstraint::Numeric),
        ))
    }

    pub fn push_error_expression(&mut self, span: RelativeSpan) -> TypedExprID {
        let ty = self.new_type_inference();
        let expression = TypedExpr::new_error(span, ty);

        self.insert_expression(expression)
    }

    pub fn push_error_expression_with_children(
        &mut self,
        span: RelativeSpan,
        children: Vec<TypedExprID>,
    ) -> TypedExprID {
        let ty = self.new_type_inference();
        let expression = TypedExpr::new_error_with_children(children, span, ty);

        self.insert_expression(expression)
    }

    pub fn span_of_expression(&self, id: TypedExprID) -> RelativeSpan {
        self.function_map.get_expression_in(self.building_function, id).span()
    }

    pub fn span_of_local_expression(&self, id: FunctionLocalID<TypedExprID>) -> RelativeSpan {
        self.function_map.get_expression_in(id.function_id(), id.local_id()).span()
    }

    pub fn push_statement(&mut self, statement: Statement) {
        self.function_map.push_statement_into(self.building_function, statement);
    }

    pub async fn resolve_function_id(&mut self, name: &Identifier) -> Option<GlobalSymbolID> {
        let root_module_id =
            self.engine().get_target_root_module_id(self.current_def_id.target_id).await;

        let members = self
            .engine
            .get_members(self.current_def_id.target_id.make_global(root_module_id))
            .await;

        if let Some(id) = members.get_by_name(&name.kind) {
            Some(self.current_def_id.target_id.make_global(id))
        } else {
            self.push_diagnostic(Diagnostic::FunctionNotFound(
                FunctionNotFound::builder().name(name.kind.0.clone()).span(name.span).build(),
            ));

            None
        }
    }

    #[must_use]
    pub async fn return_type_of_current_function(&self) -> Interned<Ty> {
        self.engine.get_return_type(self.current_def_id).await
    }

    #[must_use]
    pub async fn parameter_map_of_current_function(&self) -> Interned<ParameterMap> {
        self.engine.get_parameter_map(self.current_def_id).await
    }

    #[must_use]
    pub async fn parameter_list_syntax_of_current_function(&self) -> Option<ParameterList> {
        self.engine.get_def_signature_syntax(self.current_def_id).await.0
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
    pub fn finish(mut self) -> (FunctionMap, Vec<Diagnostic>) {
        assert!(
            self.suspended_functions.is_empty(),
            "all suspended functions should be restored before finishing the typed AST"
        );
        self.validate_lvalue_requirements();

        let (constr_diags, subst) = self.constraint_solver.residual_into_diags(&self.engine);

        self.diagnostics.extend(constr_diags);
        self.function_map.apply_mut_subst(&subst, &self.engine);

        (self.function_map, self.diagnostics)
    }
}
