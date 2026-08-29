use qbice::storage::intern::Interned;
use rayc_lexical::tree::RelativeSpan;
use rayc_type::{
    constraint::{Constraint, subtype::Subtype},
    subst::Substitutable,
    ty::{Ty, TyKind},
};
use rayc_typed_ast::{
    typed_expr::{SubExprs, TypedExprID},
    typed_function::TypedFunctionLocalID,
};

use super::{
    Cause, EffectIntroductionConstraintOrigin, EffectSharingConstraintOrigin, PendingConstraint,
    RootCause, RootCauseOrigin, SubtypeConstraintOrigin, SubtypeSource,
};
use crate::tast_builder::TAstBuilder;

impl TAstBuilder {
    pub fn push_effect_introduction(
        &mut self,
        expression_id: TypedExprID,
        introduced_effect: &Interned<Ty>,
    ) {
        let cause_id = self.constraint_solver.causes.insert(Cause::Root(RootCause {
            origin: RootCauseOrigin::EffectIntroduction(EffectIntroductionConstraintOrigin {
                expression_id: TypedFunctionLocalID::new(self.building_function, expression_id),
                span: self.span_of_expression(expression_id),

                // we don't pass the oppened effect here because we want to keep the original effect
                // row for the diagnostic
                introduced_effect: introduced_effect.clone(),
            }),
        }));

        let expr_effect = self.effect_of_expression(expression_id).clone();
        let introduced_effect = introduced_effect.clone();

        // try to open a closed row like `{IO, Exn}` to `{IO, Exn | ?X}` so that it can
        // unify with other effects
        let introduced_effect =
            Ty::open_closed_row(&introduced_effect, &mut self.constraint_solver, &self.engine)
                .unwrap_or(introduced_effect);

        let pending_constraint = PendingConstraint {
            constraint: Constraint::Subtype(Subtype::new(introduced_effect, expr_effect)),
            cause_id,
        };

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

            let cause_id = self.constraint_solver.causes.insert(Cause::Root(RootCause {
                origin: RootCauseOrigin::EffectSharing(EffectSharingConstraintOrigin {
                    child_expr_id: TypedFunctionLocalID::new(self.building_function, sub_expr),
                    parent_expr_id: TypedFunctionLocalID::new(self.building_function, dest_expr),
                }),
            }));

            let pending_constraint = PendingConstraint {
                constraint: Constraint::Subtype(Subtype::new(sub_eff, dest_eff.clone())),
                cause_id,
            };

            constraints.push(pending_constraint);
        }

        self.push_constraints(constraints);
    }

    pub fn new_effect_inference(&mut self) -> Interned<Ty> {
        self.new_type_inference_with_kind(TyKind::EffectRow)
    }

    pub fn latest_type(&self, ty: &Interned<Ty>) -> Interned<Ty> {
        ty.apply_subst_or_clone(&self.constraint_solver.subst, &self.engine)
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

        let cause = Cause::Root(RootCause {
            origin: RootCauseOrigin::Subtype(SubtypeConstraintOrigin {
                original_subtype: subtype.clone(),
                source,
                span,
            }),
        });

        let cause_id = self.constraint_solver.causes.insert(cause);
        let pending_constraint =
            PendingConstraint { constraint: Constraint::Subtype(subtype), cause_id };

        self.push_constraint(pending_constraint);
    }
}
