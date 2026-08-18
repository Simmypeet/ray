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
    function::Function,
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
    building_function: Function,
    current_function_id: GlobalSymbolID,

    name_env: NameEnv,
    solver: Solver,

    constraint_solver: ConstraintSolver,
    lvalue_requirements: LvalueRequirements,

    diagnostics: Vec<Diagnostic>,
    engine: TrackedEngine,
}

impl TAstBuilder {
    #[must_use]
    pub fn new(engine: TrackedEngine, current_function_id: GlobalSymbolID) -> Self {
        let building_function = Function::default();
        let name_env = NameEnv::new(building_function.parameter_name_binding_group_id());

        Self {
            building_function,
            name_env,
            current_function_id,
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
        self.building_function.get_expression(id).ty().clone()
    }

    #[must_use]
    pub fn type_of_name_binding(&self, name_binding_id: NameBindingID) -> Interned<Ty> {
        self.building_function.get_name_binding(name_binding_id).ty().clone()
    }

    #[must_use]
    pub fn insert_variable(&mut self, variable: Variable) -> VariableID {
        self.building_function.insert_variable(variable)
    }

    #[must_use]
    pub const fn parameter_name_binding_group(&self) -> NameBindingGroupID {
        self.building_function.parameter_name_binding_group_id()
    }

    #[must_use]
    pub const fn engine(&self) -> &TrackedEngine { &self.engine }

    /// Pushes an expression into the current block of the function being
    /// built, and returns the [`TypedExprID`] of the expression in the
    /// function's expression map.
    #[must_use]
    pub fn insert_expression(&mut self, expr: TypedExpr) -> TypedExprID {
        self.building_function.insert_expression(expr)
    }

    pub fn push_diagnostic(&mut self, diagnostic: Diagnostic) { self.diagnostics.push(diagnostic); }

    pub fn new_type_inference(&mut self) -> Interned<Ty> {
        self.engine.intern(Ty::Inference(self.solver.new_inference(TyKind::Star)))
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
        self.building_function.get_expression(id).span()
    }

    pub fn push_statement(&mut self, statement: Statement) {
        self.building_function.push_statement(statement);
    }

    pub async fn resolve_function_id(&mut self, name: &Identifier) -> Option<GlobalSymbolID> {
        let root_module_id =
            self.engine().get_target_root_module_id(self.current_function_id.target_id).await;

        let members = self
            .engine
            .get_members(self.current_function_id.target_id.make_global(root_module_id))
            .await;

        if let Some(id) = members.get_by_name(&name.kind) {
            Some(self.current_function_id.target_id.make_global(id))
        } else {
            self.push_diagnostic(Diagnostic::FunctionNotFound(
                FunctionNotFound::builder().name(name.kind.0.clone()).span(name.span).build(),
            ));

            None
        }
    }

    #[must_use]
    pub async fn return_type_of_current_function(&self) -> Interned<Ty> {
        self.engine.get_return_type(self.current_function_id).await
    }

    #[must_use]
    pub async fn parameter_map_of_current_function(&self) -> Interned<ParameterMap> {
        self.engine.get_parameter_map(self.current_function_id).await
    }

    #[must_use]
    pub async fn parameter_list_syntax_of_current_function(&self) -> Option<ParameterList> {
        self.engine.get_def_signature_syntax(self.current_function_id).await.0
    }

    #[must_use]
    pub async fn def_body_syntax_of_current_function(
        &self,
    ) -> Option<rayc_syntax::statement::Block> {
        self.engine.get_def_body_syntax(self.current_function_id).await
    }
}

impl TAstBuilder {
    #[must_use]
    pub fn finish(mut self) -> (Function, Vec<Diagnostic>) {
        self.validate_lvalue_requirements();

        let (constr_diags, subst) = self.constraint_solver.residual_into_diags(&self.engine);

        self.diagnostics.extend(constr_diags);
        self.building_function.apply_mut_subst(&subst, &self.engine);

        (self.building_function, self.diagnostics)
    }
}
