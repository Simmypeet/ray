use qbice::storage::intern::Interned;
use rayc_lexical::tree::RelativeSpan;
use rayc_type::{
    constraint::{Constraint, subtype::Subtype},
    ty::{Ty, TyKind},
};
use rayc_typed_ast::{
    statement::Statement,
    typed_expr::{SubExprs, TypedExprID},
    typed_function::TypedFunctionLocalID,
};

use crate::tast_builder::{
    TAstBuilder,
    constraint_solver::{
        provenance::{
            EffectIntroductionConstraintOrigin, EffectSharingConstraintOrigin,
            SubtypeConstraintOrigin, SubtypeSource,
        },
        solve::PendingConstraint,
    },
};

impl TAstBuilder {
    pub fn push_effect_introduction(
        &mut self,
        expression_id: TypedExprID,
        introduced_effect: &Interned<Ty>,
    ) {
        let cause_id = self.constraint_solver.provenance.insert_root_cause(
            EffectIntroductionConstraintOrigin::builder()
                .expression_id(TypedFunctionLocalID::new(self.building_function, expression_id))
                .span(self.span_of_expression(expression_id))
                // we use the original introduced effect here because we want to track the original
                // effect that was introduced, not the potentially opened version of it
                .introduced_effect(introduced_effect.clone())
                .build(),
        );

        let expr_effect = self.effect_of_expression(expression_id).clone();
        let introduced_effect = introduced_effect.clone();

        // try to open a closed row like `{IO, Exn}` to `{IO, Exn | ?X}` so that it can
        // unify with other effects
        let introduced_effect =
            Ty::open_closed_row(&introduced_effect, &mut self.constraint_solver, &self.engine)
                .unwrap_or(introduced_effect);

        let pending_constraint = PendingConstraint::builder()
            .constraint(Constraint::Subtype(Subtype::new(introduced_effect, expr_effect)))
            .cause_id(cause_id)
            .build();

        self.push_constraint(pending_constraint);
    }

    pub(in crate::tast_builder) fn compose_effect_from_sub_exprs(
        &mut self,
        dest_expr: TypedExprID,
    ) {
        // PERF: can we avoid collecting the sub-expressions into a vector?
        let sub_exprs = self.get_expression(dest_expr).kind().sub_exprs().collect::<Vec<_>>();

        let mut constraints = Vec::new();
        let dest_eff = self.effect_of_expression(dest_expr).clone();

        for sub_expr in sub_exprs {
            let sub_eff = self.effect_of_expression(sub_expr).clone();

            let cause_id = self.constraint_solver.provenance.insert_root_cause(
                EffectSharingConstraintOrigin::builder()
                    .span(self.span_of_expression(sub_expr))
                    .effect(sub_eff.clone())
                    .build(),
            );

            let pending_constraint = PendingConstraint::builder()
                .constraint(Constraint::Subtype(Subtype::new(sub_eff, dest_eff.clone())))
                .cause_id(cause_id)
                .build();

            constraints.push(pending_constraint);
        }

        self.push_constraints(constraints);
    }

    pub(in crate::tast_builder) fn compose_effect_from_statement(&mut self, statement: &Statement) {
        let expression = match statement {
            Statement::Let(statement) => statement.expression(),
            Statement::Expression(expression) => Some(*expression),
            Statement::Return(statement) => statement.value(),
        };

        let Some(expression) = expression else {
            return;
        };
        let expression_effect = self.effect_of_expression(expression).clone();
        let function_effect = self.function_map.effect_of(self.current_typed_function_id()).clone();
        let cause_id = self.constraint_solver.provenance.insert_root_cause(
            EffectSharingConstraintOrigin::builder()
                .span(self.span_of_expression(expression))
                .effect(expression_effect.clone())
                .build(),
        );

        self.push_constraint(
            PendingConstraint::builder()
                .constraint(Constraint::Subtype(Subtype::new(expression_effect, function_effect)))
                .cause_id(cause_id)
                .build(),
        );
    }

    pub(crate) async fn push_function_effect_constraint(
        &mut self,
        function_name_span: RelativeSpan,
    ) {
        let body_effect = self.function_map.effect_of(self.current_typed_function_id()).clone();
        let signature_effect = self.effect_row_of_current_function().await;

        self.push_subtype_constraint(
            &body_effect,
            &signature_effect,
            function_name_span,
            SubtypeSource::FunctionBodyEffect,
        );
    }

    pub fn new_effect_inference(&mut self) -> Interned<Ty> {
        self.new_type_inference_with_kind(TyKind::EffectRow)
    }

    pub fn push_variable_assignment_constraint(
        &mut self,
        expected_ty: &Interned<Ty>,
        expression: TypedExprID,
    ) {
        self.push_subtype_constraint_with_expr(
            expression,
            expected_ty,
            SubtypeSource::VariableAssignment,
        );
    }

    pub async fn push_return_type_constraint(&mut self, expression: TypedExprID) {
        self.push_subtype_constraint_with_expr(
            expression,
            &self.return_type_of_current_function().await,
            SubtypeSource::ReturnType,
        );
    }

    pub async fn push_unit_return_type_constraint(&mut self, span: RelativeSpan) {
        let unit_ty = Ty::new_unit(self.engine());
        self.push_subtype_constraint(
            &unit_ty,
            &self.return_type_of_current_function().await,
            span,
            SubtypeSource::ReturnType,
        );
    }

    pub fn push_function_call_constraint(
        &mut self,
        expected_ty: &Interned<Ty>,
        expression: TypedExprID,
    ) {
        self.push_subtype_constraint_with_expr(
            expression,
            expected_ty,
            SubtypeSource::FunctionCall,
        );
    }

    pub fn push_lambda_invocation_constraint(
        &mut self,
        expected_ty: &Interned<Ty>,
        expression: TypedExprID,
    ) {
        self.push_subtype_constraint_with_expr(
            expression,
            expected_ty,
            SubtypeSource::LambdaInvocation,
        );
    }

    pub fn push_binary_operator_constraint(
        &mut self,
        expected_ty: &Interned<Ty>,
        expression: TypedExprID,
    ) {
        self.push_subtype_constraint_with_expr(
            expression,
            expected_ty,
            SubtypeSource::BinaryOperator,
        );
    }

    pub fn push_if_condition_constraint(
        &mut self,
        expected_ty: &Interned<Ty>,
        expression: TypedExprID,
    ) {
        self.push_subtype_constraint_with_expr(expression, expected_ty, SubtypeSource::IfCondition);
    }

    pub fn push_if_branch_constraint(
        &mut self,
        expected_ty: &Interned<Ty>,
        expression: TypedExprID,
    ) {
        self.push_subtype_constraint_with_expr(expression, expected_ty, SubtypeSource::IfBranch);
    }

    fn push_subtype_constraint_with_expr(
        &mut self,
        arg: TypedExprID,
        expected_ty: &Interned<Ty>,
        source: SubtypeSource,
    ) {
        let ty_of_expression = self.type_of_expression(arg);
        self.push_subtype_constraint(
            &ty_of_expression,
            expected_ty,
            self.span_of_expression(arg),
            source,
        );
    }

    fn push_subtype_constraint(
        &mut self,
        actual_ty: &Interned<Ty>,
        expected_ty: &Interned<Ty>,
        span: RelativeSpan,
        source: SubtypeSource,
    ) {
        let subtype = Subtype::new(expected_ty.clone(), actual_ty.clone());

        let cause_id = self.constraint_solver.provenance.insert_root_cause(
            SubtypeConstraintOrigin::builder()
                .original_subtype(subtype.clone())
                .source(source)
                .span(span)
                .build(),
        );

        let pending_constraint = PendingConstraint::builder()
            .constraint(Constraint::Subtype(subtype))
            .cause_id(cause_id)
            .build();

        self.push_constraint(pending_constraint);
    }
}
