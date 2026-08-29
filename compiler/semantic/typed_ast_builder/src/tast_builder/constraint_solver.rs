use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_arena::{Arena, ID};
use rayc_hash::{FxHashMap, FxHashSet};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;
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
    diagnostic::{Diagnostic, IncompatibleEffectInstantiations, ResidualSubtype},
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
    span: RelativeSpan,
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
pub enum ExplanationRule {
    ConstraintDerivation(DerivationRule),

    /// The first parent is the constraint that was rewritten. The remaining
    /// parents explain the inference bindings used to rewrite it.
    AppliedSubstitution,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DerivedCause {
    rule: ExplanationRule,

    // Parent order is significant: the first parent is the primary cause used
    // for ordinary diagnostics; later parents are contributing explanations.
    parent_causes: Vec<CauseID>,
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

    // `subst` stores what each inference is equal to; this parallel map stores
    // why that binding exists. For example:
    //
    //   subst:        ?parent -> {State[int32] | ?tail}
    //   subst_causes: ?parent -> the introduction/sharing path from `intCall()`
    //
    // Keeping the cause separately lets a later constraint rewritten through
    // `?parent` retain the source of `State[int32]` in its explanation.
    subst_causes: FxHashMap<Inference, CauseID>,
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
            subst_causes: FxHashMap::default(),
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

    pub(super) fn compose_effect_from_sub_exprs(&mut self, dest_expr: TypedExprID) {
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

    fn push_constraint(&mut self, constr: PendingConstraint) {
        self.push_constraints(vec![constr]);
    }

    fn register_derived_constraint(
        &mut self,
        parent_cause: CauseID,
        derived_constraint: DerivedConstraint,
    ) -> PendingConstraint {
        let cause_id = self.constraint_solver.insert_derived_cause(
            ExplanationRule::ConstraintDerivation(derived_constraint.rule),
            vec![parent_cause],
        );

        PendingConstraint { constraint: derived_constraint.constraint, cause_id }
    }

    fn push_constraints(&mut self, queued: Vec<PendingConstraint>) {
        let mut normalized = Vec::with_capacity(queued.len());
        for pending_constraint in queued {
            normalized.push(
                self.constraint_solver
                    .apply_current_subst_or_original(pending_constraint, &self.engine),
            );
        }
        let mut queued = normalized;

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
                    let binding_cause = pending_constraint.cause_id;
                    self.move_constraints_from_residual(&subst, binding_cause, &mut queued);

                    for queued_constraint in &mut queued {
                        if let Some(new_constraint) = self.constraint_solver.apply_subst(
                            queued_constraint,
                            &subst,
                            binding_cause,
                            &self.engine,
                        ) {
                            *queued_constraint = new_constraint;
                        }
                    }

                    self.constraint_solver.compose_subst(&subst, binding_cause, &self.engine);
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
        binding_cause: CauseID,
        queued: &mut Vec<PendingConstraint>,
    ) {
        let mut i = 0;

        while i < self.constraint_solver.residual_constraints.len() {
            let pending_constraint = self.constraint_solver.residual_constraints[i].clone();
            let new_constraint = self.constraint_solver.apply_subst(
                &pending_constraint,
                subst,
                binding_cause,
                &self.engine,
            );

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
    fn insert_derived_cause(
        &mut self,
        rule: ExplanationRule,
        mut parent_causes: Vec<CauseID>,
    ) -> CauseID {
        let mut seen = FxHashSet::default();
        parent_causes.retain(|cause_id| seen.insert(*cause_id));
        self.causes.insert(Cause::Derived(DerivedCause { rule, parent_causes }))
    }

    fn constraint_has_inference_variable(constraint: &Constraint, inference: &Inference) -> bool {
        match constraint {
            Constraint::Subtype(subtype) => {
                subtype.lesser().has_inference_variable(inference)
                    || subtype.greater().has_inference_variable(inference)
            }
        }
    }

    fn apply_subst_with_causes(
        &mut self,
        pending_constraint: &PendingConstraint,
        subst: &Subst,
        binding_causes: &[(Inference, CauseID)],
        engine: &TrackedEngine,
    ) -> Option<PendingConstraint> {
        // Example:
        //
        //   pending constraint B: {State[bool] | ?b} ~ ?parent
        //   known binding A:      ?parent := {State[int32] | ?a}
        //
        // Applying A produces `{State[bool] | ?b} ~ {State[int32] | ?a}`.
        // Its explanation must be `AppliedSubstitution(B, A)`: B tells us where
        // `State[bool]` came from, while A leads back to `State[int32]`.
        let constraint = pending_constraint.constraint.apply_subst(subst, engine)?;
        let mut parent_causes = vec![pending_constraint.cause_id];

        // Only attach causes for bindings that occur in this constraint. Other
        // entries in the substitution are unrelated and would add diagnostic
        // noise if they were included.
        parent_causes.extend(binding_causes.iter().filter_map(|(inference, cause_id)| {
            Self::constraint_has_inference_variable(&pending_constraint.constraint, inference)
                .then_some(*cause_id)
        }));
        let cause_id =
            self.insert_derived_cause(ExplanationRule::AppliedSubstitution, parent_causes);

        Some(PendingConstraint { constraint, cause_id })
    }

    fn apply_subst(
        &mut self,
        pending_constraint: &PendingConstraint,
        subst: &Subst,
        binding_cause: CauseID,
        engine: &TrackedEngine,
    ) -> Option<PendingConstraint> {
        // Every mapping in this newly produced substitution came from the same
        // solver step, and therefore has the same cause.
        let binding_causes = subst
            .inference_mappings()
            .map(|(inference, _)| (inference, binding_cause))
            .collect::<Vec<_>>();
        self.apply_subst_with_causes(pending_constraint, subst, &binding_causes, engine)
    }

    fn apply_current_subst_or_original(
        &mut self,
        pending_constraint: PendingConstraint,
        engine: &TrackedEngine,
    ) -> PendingConstraint {
        // Normalize newly enqueued constraints here, where applying the current
        // substitution can also attach provenance. Applying `latest_type` before
        // creating the PendingConstraint would change the type but lose its cause.
        let subst = self.subst.clone();
        let binding_causes = subst
            .inference_mappings()
            .filter_map(|(inference, _)| {
                self.subst_causes.get(&inference).map(|cause_id| (inference, *cause_id))
            })
            .collect::<Vec<_>>();

        self.apply_subst_with_causes(&pending_constraint, &subst, &binding_causes, engine)
            .unwrap_or(pending_constraint)
    }

    fn compose_subst(&mut self, subst: &Subst, binding_cause: CauseID, engine: &TrackedEngine) {
        // Provenance must compose along with types. If the existing substitution
        // is `?a := ?b` with cause A, and this solver step adds `?b := T` with
        // cause B, the composed `?a := T` depends on both A and B.
        let new_inferences =
            subst.inference_mappings().map(|(inference, _)| inference).collect::<Vec<_>>();
        let existing_bindings = self
            .subst
            .inference_mappings()
            .filter_map(|(inference, ty)| {
                self.subst_causes.get(&inference).map(|cause_id| (inference, ty.clone(), *cause_id))
            })
            .collect::<Vec<_>>();

        for (inference, ty, existing_cause) in existing_bindings {
            if new_inferences.iter().any(|new_inference| ty.has_inference_variable(new_inference)) {
                let cause_id =
                    self.insert_derived_cause(ExplanationRule::AppliedSubstitution, vec![
                        existing_cause,
                        binding_cause,
                    ]);
                self.subst_causes.insert(inference, cause_id);
            }
        }

        for inference in new_inferences {
            self.subst_causes.entry(inference).or_insert(binding_cause);
        }
        self.subst.compose(subst, engine);
    }

    fn group_constraints_by_primary_root_cause(
        &self,
        cause_ids: impl IntoIterator<Item = CauseID>,
    ) -> FxHashSet<CauseID> {
        let mut root_cause_ids = FxHashSet::default();

        for cause_id in cause_ids {
            root_cause_ids.insert(self.primary_root_cause_id(cause_id));
        }

        root_cause_ids
    }

    fn primary_root_cause_id(&self, mut cause_id: CauseID) -> CauseID {
        loop {
            match &self.causes[cause_id] {
                Cause::Root(_root) => return cause_id,
                Cause::Derived(derived_cause) => {
                    cause_id = *derived_cause
                        .parent_causes
                        .first()
                        .expect("a derived cause should have a parent cause");
                }
            }
        }
    }

    fn collect_root_cause_ids(&self, cause_id: CauseID, roots: &mut FxHashSet<CauseID>) {
        match self.causes.get(cause_id) {
            Some(Cause::Root(_root)) => {
                roots.insert(cause_id);
            }
            Some(Cause::Derived(derived_cause)) => {
                for parent_cause in &derived_cause.parent_causes {
                    self.collect_root_cause_ids(*parent_cause, roots);
                }
            }
            None => {}
        }
    }

    fn find_effect_symbol_in_cause(&self, cause_id: CauseID) -> Option<GlobalSymbolID> {
        match self.causes.get(cause_id) {
            Some(Cause::Root(_root)) => None,
            Some(Cause::Derived(derived_cause)) => {
                match derived_cause.rule {
                    ExplanationRule::ConstraintDerivation(
                        DerivationRule::EffectLabelArgumentMatching {
                            effect_symbol_id,
                            argument_index: _argument_index,
                        },
                    ) => return Some(effect_symbol_id),
                    ExplanationRule::ConstraintDerivation(
                        DerivationRule::TypeApplicationMatching,
                    )
                    | ExplanationRule::AppliedSubstitution => {}
                }

                derived_cause
                    .parent_causes
                    .iter()
                    .find_map(|parent| self.find_effect_symbol_in_cause(*parent))
            }
            None => None,
        }
    }

    fn effect_introduction_sites(
        &self,
        cause_id: CauseID,
        effect_symbol_id: GlobalSymbolID,
        engine: &TrackedEngine,
    ) -> Vec<(RelativeSpan, Interned<Ty>)> {
        let mut roots = FxHashSet::default();
        self.collect_root_cause_ids(cause_id, &mut roots);
        let mut sites = Vec::new();

        for root_id in roots {
            let Cause::Root(root) = &self.causes[root_id] else {
                continue;
            };
            let RootCauseOrigin::EffectIntroduction(origin) = &root.origin else {
                continue;
            };

            let mut introduced_effect =
                origin.introduced_effect.apply_subst_or_clone(&self.subst, engine);
            while let Some(reduced) = introduced_effect.reduce(engine) {
                introduced_effect = reduced;
            }
            let Ty::EffectRow(effect_row) = &*introduced_effect else {
                continue;
            };

            sites.extend(
                effect_row
                    .labels()
                    .filter(|label| label.effect_symbol_id() == effect_symbol_id)
                    .map(|label| (origin.span, Ty::new_effect_row([label.clone()], None, engine))),
            );
        }

        sites.sort_by_key(|(span, _)| *span);
        sites.dedup();
        sites
    }

    fn incompatible_effect_diagnostic(
        &self,
        pending_constraint: &PendingConstraint,
        engine: &TrackedEngine,
    ) -> Option<Diagnostic> {
        let effect_symbol_id = self.find_effect_symbol_in_cause(pending_constraint.cause_id)?;
        let sites =
            self.effect_introduction_sites(pending_constraint.cause_id, effect_symbol_id, engine);

        for (index, (first_span, first_effect)) in sites.iter().enumerate() {
            for (second_span, second_effect) in &sites[index + 1..] {
                if first_effect == second_effect {
                    continue;
                }

                return Some(Diagnostic::IncompatibleEffectInstantiations(
                    IncompatibleEffectInstantiations::builder()
                        .first_span(*first_span)
                        .first_effect(first_effect.clone())
                        .second_span(*second_span)
                        .second_effect(second_effect.clone())
                        .build(),
                ));
            }
        }

        None
    }

    #[must_use]
    pub fn residual_into_diags(mut self, engine: &TrackedEngine) -> (Vec<Diagnostic>, Subst) {
        let root_cause_ids = self.group_constraints_by_primary_root_cause(
            self.residual_constraints
                .iter()
                .map(|x| x.cause_id)
                .chain(self.errored_constraints.iter().map(|x| x.1.cause_id)),
        );

        let mut diags = Vec::new();
        for (_, pending) in &self.errored_constraints {
            if let Some(diagnostic) = self.incompatible_effect_diagnostic(pending, engine)
                && !diags.contains(&diagnostic)
            {
                diags.push(diagnostic);
            }
        }
        diags.extend(root_cause_ids.into_iter().filter_map(|cause_id| {
            match &self.causes[cause_id] {
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

                    RootCauseOrigin::EffectIntroduction(_) | RootCauseOrigin::EffectSharing(_) => {
                        None
                    }
                },
                Cause::Derived(_derived) => None,
            }
        }));

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
