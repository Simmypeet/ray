use derive_more::From;
use qbice::storage::intern::Interned;
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_resolution::{Obligation, PredicateConstraint};
use rayc_type::{
    constraint::{instance_trait_ref::InstanceTraitRef, ty_relate::TyRelate},
    reduce::Reduce,
    subst::Substitutable,
    trait_ref::TraitRef,
    ty::{Ty, TyKind, effect_row::EffectLabel},
    where_clause::MarkerPredicate,
};
use rayc_typed_ast::{
    statement::Statement,
    typed_expr::{SubExprs, TypedExprID},
    typed_function::TypedFunctionID,
};

use crate::tast_builder::{
    TAstBuilder,
    constraint_solver::{
        provenance::{
            EffectUnificationOrigin, EffectUnificationSource, SubtypeConstraintOrigin,
            SubtypeSource,
        },
        solve::PendingConstraint,
    },
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, From)]
pub enum Constraint {
    InstanceTraitRef(InstanceTraitRef),
    MarkerPredicate(MarkerPredicate),
    TyRelate(TyRelate),
    InstanceResolve { instance: Interned<Ty>, trait_ref: TraitRef },
}

impl Reduce for Constraint {
    async fn reduce(
        &self,
        engine: &TrackedEngine,
        givens: &[rayc_type::where_clause::PredicateKind],
    ) -> Option<Self>
    where
        Self: Sized,
    {
        match self {
            Self::InstanceTraitRef(check) => {
                check.reduce(engine, givens).await.map(Self::InstanceTraitRef)
            }
            Self::TyRelate(ty_relate) => {
                ty_relate.reduce(engine, givens).await.map(Constraint::TyRelate)
            }
            Self::MarkerPredicate(predicate) => {
                predicate.implementor().reduce(engine, givens).await.map(|implementor| {
                    Self::MarkerPredicate(MarkerPredicate::new(predicate.marker_id(), implementor))
                })
            }
            Self::InstanceResolve { instance, trait_ref } => {
                match (
                    instance.reduce(engine, givens).await,
                    trait_ref.reduce(engine, givens).await,
                ) {
                    (None, None) => None,
                    (new_instance, new_trait_ref) => Some(Self::InstanceResolve {
                        instance: new_instance.unwrap_or_else(|| instance.clone()),
                        trait_ref: new_trait_ref.unwrap_or_else(|| trait_ref.clone()),
                    }),
                }
            }
        }
    }
}

impl Substitutable for Constraint {
    fn apply_subst(&self, subst: &rayc_type::subst::Subst, engine: &TrackedEngine) -> Option<Self>
    where
        Self: Sized,
    {
        match self {
            Self::InstanceResolve { instance, trait_ref } => {
                match (instance.apply_subst(subst, engine), trait_ref.apply_subst(subst, engine)) {
                    (None, None) => None,
                    (new_instance, new_trait_ref) => Some(Self::InstanceResolve {
                        instance: new_instance.unwrap_or_else(|| instance.clone()),
                        trait_ref: new_trait_ref.unwrap_or_else(|| trait_ref.clone()),
                    }),
                }
            }
            Self::InstanceTraitRef(check) => {
                check.apply_subst(subst, engine).map(Self::InstanceTraitRef)
            }
            Self::TyRelate(ty_relate) => {
                ty_relate.apply_subst(subst, engine).map(Constraint::TyRelate)
            }
            Self::MarkerPredicate(predicate) => {
                predicate.apply_subst(subst, engine).map(Self::MarkerPredicate)
            }
        }
    }
}

impl Constraint {
    pub fn interned_recursive_iter(&self) -> Box<dyn Iterator<Item = &Interned<Ty>> + '_> {
        match self {
            Self::InstanceTraitRef(check) => Box::new(check.interned_recursive_iter()),
            Self::TyRelate(ty_relate) => Box::new(ty_relate.interned_recursive_iter()),
            Self::MarkerPredicate(predicate) => {
                Box::new(Ty::interned_recursive_iter(predicate.implementor()))
            }
            Self::InstanceResolve { instance, trait_ref } => Box::new(
                Ty::interned_recursive_iter(instance)
                    .chain(trait_ref.args().interned_iter().flat_map(Ty::interned_recursive_iter)),
            ),
        }
    }
}

impl TAstBuilder {
    pub async fn push_resolution_obligations(
        &mut self,
        obligations: impl IntoIterator<Item = Obligation>,
    ) {
        for ob in obligations {
            match ob {
                Obligation::TraitRefCheck(trait_ref_check) => {
                    let root_cause_id = self
                        .constraint_solver
                        .provenance
                        .insert_root_cause(trait_ref_check.clone());

                    self.push_constraint(
                        PendingConstraint::builder()
                            .constraint(trait_ref_check.into_constraint().into())
                            .cause_id(root_cause_id)
                            .build(),
                    )
                    .await;
                }
                Obligation::WfCheck(check) => {
                    for predicate in check.predicate_obligations(&self.engine).await {
                        let constraint = match predicate.constraint() {
                            PredicateConstraint::TyRelate(constraint) => {
                                Constraint::TyRelate(constraint)
                            }
                            PredicateConstraint::Marker(marker) => {
                                Constraint::MarkerPredicate(marker)
                            }
                        };
                        let root_cause_id =
                            self.constraint_solver.provenance.insert_root_cause(predicate.clone());

                        self.push_constraint(
                            PendingConstraint::builder()
                                .constraint(constraint)
                                .cause_id(root_cause_id)
                                .build(),
                        )
                        .await;
                    }
                }
            }
        }
    }

    pub async fn push_effect_introduction(
        &mut self,
        expression_id: TypedExprID,
        introduced_effect: &Interned<Ty>,
    ) {
        let expr_effect = self.effect_of_expression(expression_id).clone();
        let introduced_effect = introduced_effect.clone();

        // try to open a closed row like `{IO, Exn}` to `{IO, Exn | ?X}` so that it can
        // unify with other effects
        let original_effect = introduced_effect.clone();
        let introduced_effect =
            Ty::open_closed_row(&introduced_effect, &mut self.constraint_solver, &self.engine)
                .unwrap_or(introduced_effect);

        self.push_effect_unification_constraint(
            introduced_effect,
            expr_effect,
            self.span_of_expression(expression_id),
            // we pass the original effect before it's openned, so that the user can see
            // the **actual** effect before it got mixed with other effects in the unification
            // process
            EffectUnificationSource::EffectIntroduction { original_effect },
        )
        .await;
    }

    pub(in crate::tast_builder) async fn compose_effect_from_sub_exprs(
        &mut self,
        dest_expr: TypedExprID,
    ) {
        // PERF: can we avoid collecting the sub-expressions into a vector?
        let sub_exprs = self.get_expression(dest_expr).kind().sub_exprs().collect::<Vec<_>>();

        let mut constraints = Vec::new();
        let dest_eff = self.effect_of_expression(dest_expr).clone();

        for sub_expr in sub_exprs {
            let sub_eff = self.effect_of_expression(sub_expr).clone();

            let pending_constraint = self.effect_unification_constraint(
                sub_eff,
                dest_eff.clone(),
                self.span_of_expression(sub_expr),
                EffectUnificationSource::EffectSharing,
            );

            constraints.push(pending_constraint);
        }

        self.push_constraints(constraints).await;
    }

    pub(in crate::tast_builder) async fn compose_function_effect_from_statement(
        &mut self,
        statement: &Statement,
    ) {
        let function_effect = self.function_map.effect_of(self.current_typed_function_id()).clone();
        let constraints = statement
            .sub_exprs()
            .map(|expression| {
                self.effect_unification_constraint(
                    self.effect_of_expression(expression).clone(),
                    function_effect.clone(),
                    self.span_of_expression(expression),
                    EffectUnificationSource::EffectSharing,
                )
            })
            .collect();
        self.push_constraints(constraints).await;
    }

    pub(crate) async fn compose_run_with_effect(
        &mut self,
        run_with_expression: TypedExprID,
        body_function: TypedFunctionID,
        operation_handlers: impl IntoIterator<Item = TypedFunctionID>,
        handled_effect: Interned<EffectLabel>,
    ) {
        let span = self.span_of_expression(run_with_expression);
        let run_with_effect = self.effect_of_expression(run_with_expression).clone();
        let original_body_effect = self.function_map.effect_of(body_function).clone();

        let handled_body_effect = self.new_effect_inference();
        let body_effect_with_handled_label =
            Ty::new_effect_row([handled_effect], Some(handled_body_effect.clone()), self.engine());

        let mut constraints = vec![
            self.effect_unification_constraint(
                original_body_effect,
                body_effect_with_handled_label,
                span,
                EffectUnificationSource::EffectSharing,
            ),
            self.effect_unification_constraint(
                handled_body_effect,
                run_with_effect.clone(),
                span,
                EffectUnificationSource::EffectSharing,
            ),
        ];

        for operation_handler in operation_handlers {
            let operation_body_effect = self.function_map.effect_of(operation_handler).clone();
            constraints.push(self.effect_unification_constraint(
                operation_body_effect,
                run_with_effect.clone(),
                span,
                EffectUnificationSource::EffectSharing,
            ));
        }

        self.push_constraints(constraints).await;
    }

    pub(crate) async fn push_function_effect_constraint(
        &mut self,
        function_name_span: RelativeSpan,
    ) {
        let body_effect = self.function_map.effect_of(self.current_typed_function_id()).clone();
        let signature_effect = self.effect_row_of_current_function().await;

        self.push_effect_unification_constraint(
            signature_effect,
            body_effect,
            function_name_span,
            EffectUnificationSource::FunctionBodyEffect,
        )
        .await;
    }

    fn effect_unification_constraint(
        &mut self,
        lesser: Interned<Ty>,
        greater: Interned<Ty>,
        span: RelativeSpan,
        source: EffectUnificationSource,
    ) -> PendingConstraint {
        let cause_id = self.constraint_solver.provenance.insert_root_cause(
            EffectUnificationOrigin::builder()
                .lesser(lesser.clone())
                .greater(greater.clone())
                .source(source)
                .span(span)
                .build(),
        );

        PendingConstraint::builder()
            .constraint(Constraint::TyRelate(TyRelate::new(lesser, greater)))
            .cause_id(cause_id)
            .build()
    }

    async fn push_effect_unification_constraint(
        &mut self,
        lesser: Interned<Ty>,
        greater: Interned<Ty>,
        span: RelativeSpan,
        source: EffectUnificationSource,
    ) {
        let pending_constraint = self.effect_unification_constraint(lesser, greater, span, source);
        self.push_constraint(pending_constraint).await;
    }

    pub fn new_effect_inference(&mut self) -> Interned<Ty> {
        self.new_type_inference_with_kind(TyKind::EffectRow)
    }

    pub async fn push_variable_assignment_constraint(
        &mut self,
        expected_ty: &Interned<Ty>,
        expression: TypedExprID,
    ) {
        self.push_subtype_constraint_with_expr(
            expression,
            expected_ty,
            SubtypeSource::VariableAssignment,
        )
        .await;
    }

    pub async fn push_return_type_constraint(&mut self, expression: TypedExprID) {
        self.push_subtype_constraint_with_expr(
            expression,
            &self.return_type_of_current_function().await,
            SubtypeSource::ReturnType,
        )
        .await;
    }

    pub async fn push_unit_return_type_constraint(&mut self, span: RelativeSpan) {
        let unit_ty = Ty::new_unit(self.engine());
        self.push_subtype_constraint(
            &unit_ty,
            &self.return_type_of_current_function().await,
            span,
            SubtypeSource::ReturnType,
        )
        .await;
    }

    pub async fn push_function_call_constraint(
        &mut self,
        expected_ty: &Interned<Ty>,
        expression: TypedExprID,
    ) {
        self.push_subtype_constraint_with_expr(
            expression,
            expected_ty,
            SubtypeSource::FunctionCall,
        )
        .await;
    }

    pub async fn push_struct_field_initialization_constraint(
        &mut self,
        expected_ty: &Interned<Ty>,
        expression: TypedExprID,
    ) {
        self.push_subtype_constraint_with_expr(
            expression,
            expected_ty,
            SubtypeSource::StructFieldInitialization,
        )
        .await;
    }

    pub async fn push_binary_operator_constraint(
        &mut self,
        expected_ty: &Interned<Ty>,
        expression: TypedExprID,
    ) {
        self.push_subtype_constraint_with_expr(
            expression,
            expected_ty,
            SubtypeSource::BinaryOperator,
        )
        .await;
    }

    pub async fn push_if_condition_constraint(
        &mut self,
        expected_ty: &Interned<Ty>,
        expression: TypedExprID,
    ) {
        self.push_subtype_constraint_with_expr(expression, expected_ty, SubtypeSource::IfCondition)
            .await;
    }

    pub async fn push_while_condition_constraint(
        &mut self,
        expected_ty: &Interned<Ty>,
        expression: TypedExprID,
    ) {
        self.push_subtype_constraint_with_expr(
            expression,
            expected_ty,
            SubtypeSource::WhileCondition,
        )
        .await;
    }

    pub async fn push_if_branch_constraint(
        &mut self,
        expected_ty: &Interned<Ty>,
        expression: TypedExprID,
    ) {
        self.push_subtype_constraint_with_expr(expression, expected_ty, SubtypeSource::IfBranch)
            .await;
    }

    pub async fn push_if_unit_branch_constraint(
        &mut self,
        expected_ty: &Interned<Ty>,
        span: RelativeSpan,
    ) {
        self.push_subtype_constraint(
            &Ty::new_unit(self.engine()),
            expected_ty,
            span,
            SubtypeSource::IfBranch,
        )
        .await;
    }

    async fn push_subtype_constraint_with_expr(
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
        )
        .await;
    }

    pub(in crate::tast_builder) async fn push_capture_constraint(
        &mut self,
        inference: &Interned<Ty>,
        tuple: &Interned<Ty>,
        span: RelativeSpan,
    ) {
        self.push_subtype_constraint(tuple, inference, span, SubtypeSource::ClosureCaptures).await;
    }

    async fn push_subtype_constraint(
        &mut self,
        actual_ty: &Interned<Ty>,
        expected_ty: &Interned<Ty>,
        span: RelativeSpan,
        source: SubtypeSource,
    ) {
        let subtype = TyRelate::new(expected_ty.clone(), actual_ty.clone());

        let cause_id = self.constraint_solver.provenance.insert_root_cause(
            SubtypeConstraintOrigin::builder()
                .original_subtype(subtype.clone())
                .source(source)
                .span(span)
                .build(),
        );

        let pending_constraint = PendingConstraint::builder()
            .constraint(Constraint::TyRelate(subtype))
            .cause_id(cause_id)
            .build();

        self.push_constraint(pending_constraint).await;
    }
}
