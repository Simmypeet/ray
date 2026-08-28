use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_arena::{Arena, ID};
use rayc_hash::FxHashSet;
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_type::{
    constraint::{self, Constraint, DerivationRule, DerivedConstraint, Step, subtype::Subtype},
    reduce::Reduce,
    solver::Solver,
    subst::{Subst, Substitutable},
    ty::{
        InferenceConstraint, Primitive, Ty, TyKind,
        inference::{GenInfer, Inference},
    },
};
use rayc_typed_ast::{
    typed_expr::{SubExprs, TypedExprID},
    typed_function::TypedFunctionLocalID,
};

use crate::{
    diagnostic::{Diagnostic, ResidualSubtype},
    tast_builder::TAstBuilder,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum SubtypeSource {
    FunctionCall,
    LambdaInvocation,
    VariableAssignment,
    BinaryOperator,
    IfCondition,
    IfBranch,
    ReturnType,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SubtypeConstraintOrigin {
    original_subtype: Subtype,
    source: SubtypeSource,
    span: RelativeSpan,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct EffectIntroductionConstraintOrigin {
    expression_id: TypedFunctionLocalID<TypedExprID>,
    introduced_effect: Interned<Ty>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct EffectSharingConstraintOrigin {
    child_expr_id: TypedFunctionLocalID<TypedExprID>,
    parent_expr_id: TypedFunctionLocalID<TypedExprID>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum RootCauseOrigin {
    Subtype(SubtypeConstraintOrigin),
    EffectSharing(EffectSharingConstraintOrigin),
    EffectIntroduction(EffectIntroductionConstraintOrigin),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RootCause {
    origin: RootCauseOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct DerivedCause {
    derivation_rule: DerivationRule,
    parent_cause: CauseID,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Cause {
    Root(RootCause),
    Derived(DerivedCause),
}

pub type CauseID = ID<Cause>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PendingConstraint {
    constraint: Constraint,
    cause_id: CauseID,
}

impl Substitutable for PendingConstraint {
    fn apply_subst(&self, subst: &Subst, engine: &rayc_qbice::TrackedEngine) -> Option<Self>
    where
        Self: Sized,
    {
        self.constraint
            .apply_subst(subst, engine)
            .map(|new_constraint| Self { constraint: new_constraint, cause_id: self.cause_id })
    }
}

impl Reduce for PendingConstraint {
    fn reduce(&self, engine: &TrackedEngine) -> Option<Self>
    where
        Self: Sized,
    {
        self.constraint
            .reduce(engine)
            .map(|new_constraint| Self { constraint: new_constraint, cause_id: self.cause_id })
    }
}

#[derive(Debug)]
pub struct ConstraintSolver {
    causes: Arena<Cause>,

    residual_constraints: Vec<PendingConstraint>,
    errored_constraints: Vec<(constraint::Error, PendingConstraint)>,

    numeric_inferences: Vec<Inference>,

    subst: Subst,
    solver: Solver,
}

impl ConstraintSolver {
    #[must_use]
    pub fn new(engine: TrackedEngine) -> Self {
        Self {
            causes: Arena::default(),
            residual_constraints: Vec::new(),
            errored_constraints: Vec::new(),
            numeric_inferences: Vec::new(),
            subst: Subst::new_empty(),
            solver: Solver::new(engine),
        }
    }
}

impl TAstBuilder {
    pub fn push_effect_introduction(
        &mut self,
        expression_id: TypedExprID,
        introduced_effect: &Interned<Ty>,
    ) {
        let cause_id = self.constraint_solver.causes.insert(Cause::Root(RootCause {
            origin: RootCauseOrigin::EffectIntroduction(EffectIntroductionConstraintOrigin {
                expression_id: TypedFunctionLocalID::new(self.building_function, expression_id),

                // we don't pass the oppened effect here because we want to keep the original effect
                // row for the diagnostic
                introduced_effect: introduced_effect.clone(),
            }),
        }));

        let expr_effect = self.latest_type(self.effect_of_expression(expression_id));
        let introduced_effect = self.latest_type(introduced_effect);

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

    pub(super) fn compose_effect_from_sub_exprs(&mut self, dest_expr: TypedExprID) {
        // PERF: can we avoid collecting the sub-expressions into a vector?
        let sub_exprs = self.get_expression(dest_expr).kind().sub_exprs().collect::<Vec<_>>();

        let mut constraints = Vec::new();
        let dest_eff = self.latest_type(self.effect_of_expression(dest_expr));

        for sub_expr in sub_exprs {
            let sub_eff = self.latest_type(self.effect_of_expression(sub_expr));

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
        let ty_of_expression = self.latest_type(&self.type_of_expression(arg));
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
        let actual_ty = self.latest_type(actual_ty);
        let expected_ty = self.latest_type(expected_ty);
        let subtype = Subtype::new(expected_ty, actual_ty);

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

    fn push_constraint(&mut self, constr: PendingConstraint) {
        self.push_constraints(vec![constr]);
    }

    fn register_derived_constraint(
        &mut self,
        parent_cause: CauseID,
        derived_constraint: DerivedConstraint,
    ) -> PendingConstraint {
        let cause =
            Cause::Derived(DerivedCause { derivation_rule: derived_constraint.rule, parent_cause });

        let cause_id = self.constraint_solver.causes.insert(cause);

        PendingConstraint { constraint: derived_constraint.constraint, cause_id }
    }

    fn push_constraints(&mut self, mut queued: Vec<PendingConstraint>) {
        while let Some(pending_constraint) = queued.pop() {
            match self.constraint_solver.solver.entail(&pending_constraint.constraint) {
                Ok(Step::Derived(constrs)) => {
                    queued.extend(
                        constrs.into_iter().map(|x| {
                            self.register_derived_constraint(pending_constraint.cause_id, x)
                        }),
                    );
                }

                Ok(Step::Subst(subst)) => {
                    self.move_constraints_from_residual(&subst, &mut queued);

                    // NOTE! apply the substitution to the sibling constraints in the queue too
                    for queued in &mut queued {
                        queued.constraint.apply_in_place(&subst, &self.engine);
                    }

                    self.constraint_solver.subst.compose(&subst, &self.engine);
                }

                Ok(Step::NoProgress) => {
                    if let Some(reduced_constraint) = pending_constraint.reduce(&self.engine) {
                        queued.push(reduced_constraint);
                    } else {
                        self.constraint_solver.residual_constraints.push(pending_constraint);
                    }
                }

                Err(err) => {
                    self.constraint_solver.errored_constraints.push((err, pending_constraint));
                }
            }
        }
    }

    fn move_constraints_from_residual(
        &mut self,
        subst: &Subst,
        queued: &mut Vec<PendingConstraint>,
    ) {
        let mut i = 0;

        while i < self.constraint_solver.residual_constraints.len() {
            let new_constraint =
                self.constraint_solver.residual_constraints[i].apply_subst(subst, self.engine());

            match new_constraint {
                Some(new_constraint) => {
                    queued.push(new_constraint);
                    self.constraint_solver.residual_constraints.remove(i);
                }
                None => {
                    i += 1;
                }
            }
        }
    }
}

impl ConstraintSolver {
    fn group_constraints_by_root_cause(
        &self,
        cause_ids: impl IntoIterator<Item = CauseID>,
    ) -> FxHashSet<CauseID> {
        let mut root_cause_ids = FxHashSet::default();

        for cause_id in cause_ids {
            let root_cause_id = self.traverse_to_root_cause(cause_id);
            root_cause_ids.insert(root_cause_id);
        }

        root_cause_ids
    }

    fn traverse_to_root_cause(&self, mut cause_id: CauseID) -> CauseID {
        loop {
            match self.causes.get(cause_id) {
                Some(Cause::Derived(derived_cause)) => {
                    cause_id = derived_cause.parent_cause;
                }
                _ => return cause_id,
            }
        }
    }

    #[must_use]
    pub fn residual_into_diags(mut self, engine: &TrackedEngine) -> (Vec<Diagnostic>, Subst) {
        // collect all the root causes of the residual and errored constraints, so that
        // we can reduce them to ther original form
        //
        // actually, we could potentially group the residual and errored constraints by
        // their root causes like `root_cause_id -> [errored_constraint,
        // errored_constraint, ...]` and then use these information to generate more
        // informative diagnostics, but i don't know how :-P
        //
        // for now, i'll just generate somewhat generic diagnostics with at least the
        // original subtype information
        let root_cause_ids = self.group_constraints_by_root_cause(
            self.residual_constraints
                .iter()
                .map(|x| x.cause_id)
                .chain(self.errored_constraints.iter().map(|x| x.1.cause_id)),
        );

        let diags = root_cause_ids
            .into_iter()
            .filter_map(|cause_id| match &self.causes[cause_id] {
                Cause::Root(root) => match &root.origin {
                    RootCauseOrigin::Subtype(subtype_constraint_origin) => {
                        let subtype = subtype_constraint_origin
                            .original_subtype
                            .apply_subst_or_clone(&self.subst, engine);

                        Some(Diagnostic::ResidualSubtype(
                            ResidualSubtype::builder()
                                .source(subtype_constraint_origin.source)
                                .span(subtype_constraint_origin.span)
                                .subype(subtype)
                                .build(),
                        ))
                    }

                    // TODO: Implement a correct diagnostic for effect sharing errors.
                    RootCauseOrigin::EffectIntroduction(_) | RootCauseOrigin::EffectSharing(_) => {
                        None
                    }
                },
                Cause::Derived(_derived_cause) => None,
            })
            .collect();

        let int32 = Ty::new_primitive(Primitive::Int32, engine);
        let numeric_defaults = self
            .numeric_inferences
            .iter()
            .filter(|inference| self.subst.get(&**inference).is_none())
            .map(|inference| (*inference, int32.clone()))
            .collect();
        self.subst.compose(&numeric_defaults, engine);

        (diags, self.subst)
    }
}

impl GenInfer for ConstraintSolver {
    fn gen_infer(&mut self, kind: TyKind, constraint: InferenceConstraint) -> Inference {
        if kind == TyKind::Star && constraint == InferenceConstraint::Numeric {
            let inference = self.solver.new_inference_with_constraint(kind, constraint);
            self.numeric_inferences.push(inference);
            inference
        } else {
            self.solver.new_inference_with_constraint(kind, constraint)
        }
    }
}

impl TAstBuilder {
    pub fn new_type_inference(&mut self) -> Interned<Ty> {
        self.new_type_inference_with_kind(TyKind::Star)
    }

    pub fn new_type_inference_with_kind(&mut self, kind: TyKind) -> Interned<Ty> {
        let inference = self.constraint_solver.gen_infer(kind, InferenceConstraint::Any);
        self.engine.intern(Ty::Inference(inference))
    }

    pub fn new_numeric_type_inference(&mut self) -> Interned<Ty> {
        let inference =
            self.constraint_solver.gen_infer(TyKind::Star, InferenceConstraint::Numeric);
        self.engine.intern(Ty::Inference(inference))
    }

    pub fn new_equality_comparable_type_inference(&mut self) -> Interned<Ty> {
        let inference =
            self.constraint_solver.gen_infer(TyKind::Star, InferenceConstraint::EqualityComparable);
        self.engine.intern(Ty::Inference(inference))
    }
}

#[cfg(test)]
mod tests {
    use qbice::storage::intern::Interned;
    use rayc_lexical::tree::{OffsetMode, ROOT_BRANCH_ID, RelativeLocation, RelativeSpan};
    use rayc_qbice::TrackedEngine;
    use rayc_source_file::GlobalSourceID;
    use rayc_symbol::GlobalSymbolID;
    use rayc_type::{
        constraint::{Constraint, Error, subtype::Subtype},
        ty::{Primitive, Ty, args::Args},
    };

    use super::{
        Cause, PendingConstraint, RootCause, RootCauseOrigin, SubtypeConstraintOrigin,
        SubtypeSource,
    };
    use crate::tast_builder::TAstBuilder;

    fn test_span() -> RelativeSpan {
        RelativeSpan {
            start: RelativeLocation {
                offset: 0,
                mode: OffsetMode::Start,
                relative_to: ROOT_BRANCH_ID,
            },
            end: RelativeLocation { offset: 1, mode: OffsetMode::End, relative_to: ROOT_BRANCH_ID },
            source_id: GlobalSourceID::default(),
        }
    }

    fn pending_constraint(builder: &mut TAstBuilder, subtype: Subtype) -> PendingConstraint {
        let cause_id = builder.constraint_solver.causes.insert(Cause::Root(RootCause {
            origin: RootCauseOrigin::Subtype(SubtypeConstraintOrigin {
                original_subtype: subtype.clone(),
                source: SubtypeSource::IfBranch,
                span: test_span(),
            }),
        }));

        PendingConstraint { constraint: Constraint::Subtype(subtype), cause_id }
    }

    fn state_effect(
        argument: Interned<Ty>,
        effect_id: GlobalSymbolID,
        engine: &TrackedEngine,
    ) -> Interned<Ty> {
        let label = engine.intern(rayc_type::ty::effect_row::EffectLabel::new(
            effect_id,
            Args::new([argument], engine),
        ));
        Ty::new_effect_row([label], None, engine)
    }

    // input: {State[int32]} = e, {State[bool]} = e
    // premise: both constraints are queued before either one is solved
    // output: Conflicted
    #[tokio::test]
    async fn queued_constraints_observe_prior_substitutions() {
        let engine = rayc_qbice::create_minimal_engine().await;
        let mut builder = TAstBuilder::new(engine.clone(), GlobalSymbolID::default());
        let shared_effect = builder.new_effect_inference();
        let effect_id = GlobalSymbolID::default();
        let state_int32 =
            state_effect(Ty::new_primitive(Primitive::Int32, &engine), effect_id, &engine);
        let state_bool =
            state_effect(Ty::new_primitive(Primitive::Bool, &engine), effect_id, &engine);
        let int32_constraint =
            pending_constraint(&mut builder, Subtype::new(state_int32, shared_effect.clone()));
        let bool_constraint =
            pending_constraint(&mut builder, Subtype::new(state_bool, shared_effect));

        builder.push_constraints(vec![int32_constraint, bool_constraint]);

        assert_eq!(builder.constraint_solver.errored_constraints.len(), 1);
        assert_eq!(builder.constraint_solver.errored_constraints[0].0, Error::Conflicted);
    }
}
