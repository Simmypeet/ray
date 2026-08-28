use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_type::{
    constraint::{self, Constraint, Step, subtype::Subtype},
    reduce::Reduce,
    solver::Solver,
    subst::{Subst, Substitutable},
    ty::{InferenceConstraint, Primitive, Ty, TyKind, inference::Inference},
};
use rayc_typed_ast::typed_expr::{SubExprs, TypedExprID};

use crate::{
    diagnostic::{Diagnostic, ResidualSubtype},
    tast_builder::TAstBuilder,
};

/// Describes the origin of a constraint, which can be used for better error
/// reporting and debugging.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Provenance {
    Subtype(SubtypeProvenance),
    EffectCompose(EffectComposeProvenance),
}

impl Substitutable for Provenance {
    fn apply_subst(&self, subst: &Subst, engine: &rayc_qbice::TrackedEngine) -> Option<Self>
    where
        Self: Sized,
    {
        match self {
            Self::Subtype(subtype_provenance) => {
                subtype_provenance.apply_subst(subst, engine).map(Self::Subtype)
            }
            Self::EffectCompose(_) => None,
        }
    }
}

impl Reduce for Provenance {
    fn reduce(&self, engine: &TrackedEngine) -> Option<Self>
    where
        Self: Sized,
    {
        match self {
            Self::Subtype(subtype_provenance) => {
                subtype_provenance.reduce(engine).map(Self::Subtype)
            }
            Self::EffectCompose(_) => None,
        }
    }
}

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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct SubtypeProvenance {
    original_subtype: Subtype,
    span: RelativeSpan,
    source: SubtypeSource,
}

impl SubtypeProvenance {
    #[must_use]
    pub const fn original_subtype(&self) -> &Subtype { &self.original_subtype }

    #[must_use]
    pub const fn source(&self) -> SubtypeSource { self.source }

    #[must_use]
    pub const fn span(&self) -> &RelativeSpan { &self.span }
}

impl Reduce for SubtypeProvenance {
    fn reduce(&self, engine: &TrackedEngine) -> Option<Self>
    where
        Self: Sized,
    {
        self.original_subtype.reduce(engine).map(|new_subtype| Self {
            original_subtype: new_subtype,
            span: self.span,
            source: self.source,
        })
    }
}

impl Substitutable for SubtypeProvenance {
    fn apply_subst(&self, subst: &Subst, engine: &rayc_qbice::TrackedEngine) -> Option<Self>
    where
        Self: Sized,
    {
        self.original_subtype.apply_subst(subst, engine).map(|new_subtype| Self {
            original_subtype: new_subtype,
            span: self.span,
            source: self.source,
        })
    }
}

/// Representing a situation where an effect of an expression is composed of
/// multiple sub-expressions.
///
/// For instance, in a tuple `(a, b)`, the effect of the tuple is composed of
/// the effects of `a` and `b`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct EffectComposeProvenance {
    sub_exprs: Interned<[TypedExprID]>,
    dest_expr: TypedExprID,
}

/// A constraint equipped with its provenance
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProvenancedConstraint {
    provenance: Provenance,
    constraint: Constraint,
}

impl Reduce for ProvenancedConstraint {
    fn reduce(&self, engine: &TrackedEngine) -> Option<Self>
    where
        Self: Sized,
    {
        match (self.provenance.reduce(engine), self.constraint.reduce(engine)) {
            (Some(new_provenance), Some(new_constraint)) => {
                Some(Self { provenance: new_provenance, constraint: new_constraint })
            }
            (Some(new_provenance), None) => {
                Some(Self { provenance: new_provenance, constraint: self.constraint.clone() })
            }
            (None, Some(new_constraint)) => {
                Some(Self { provenance: self.provenance.clone(), constraint: new_constraint })
            }
            (None, None) => None,
        }
    }
}

impl Substitutable for ProvenancedConstraint {
    fn apply_subst(&self, subst: &Subst, engine: &rayc_qbice::TrackedEngine) -> Option<Self>
    where
        Self: Sized,
    {
        match (
            self.provenance.apply_subst(subst, engine),
            self.constraint.apply_subst(subst, engine),
        ) {
            (Some(new_provenance), Some(new_constraint)) => {
                Some(Self { provenance: new_provenance, constraint: new_constraint })
            }
            (Some(new_provenance), None) => {
                Some(Self { provenance: new_provenance, constraint: self.constraint.clone() })
            }
            (None, Some(new_constraint)) => {
                Some(Self { provenance: self.provenance.clone(), constraint: new_constraint })
            }
            (None, None) => None,
        }
    }
}

impl ProvenancedConstraint {
    #[must_use]
    pub const fn new(provenance: Provenance, constraint: Constraint) -> Self {
        Self { provenance, constraint }
    }
}

#[derive(Debug)]
pub struct ConstraintSolver {
    residual_constraints: Vec<ProvenancedConstraint>,
    errored_constraints: Vec<(constraint::Error, ProvenancedConstraint)>,
    numeric_inferences: Vec<Inference>,
    subst: Subst,
    solver: Solver,
}

impl ConstraintSolver {
    #[must_use]
    pub fn new(engine: TrackedEngine) -> Self {
        Self {
            residual_constraints: Vec::new(),
            errored_constraints: Vec::new(),
            numeric_inferences: Vec::new(),
            subst: Subst::new_empty(),
            solver: Solver::new(engine),
        }
    }
}

impl TAstBuilder {
    pub(super) fn compose_effect_from_sub_exprs(&mut self, dest_expr: TypedExprID) {
        let sub_exprs: Interned<[_]> = self
            .engine
            .intern_unsized(self.get_expression(dest_expr).kind().sub_exprs().collect::<Vec<_>>());

        let effect_compose_provenance = Provenance::EffectCompose(EffectComposeProvenance {
            sub_exprs: sub_exprs.clone(),
            dest_expr,
        });

        let mut constraints = Vec::new();
        let dest_eff = self.latest_type(self.effect_of_expression(dest_expr));

        for sub_expr in sub_exprs.iter() {
            let sub_eff = self.latest_type(self.effect_of_expression(*sub_expr));

            constraints.push(ProvenancedConstraint::new(
                effect_compose_provenance.clone(),
                Constraint::Subtype(Subtype::new(sub_eff, dest_eff.clone())),
            ));
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
        let provenance = Provenance::Subtype(SubtypeProvenance {
            original_subtype: subtype.clone(),
            span,
            source,
        });

        let constraint = Constraint::Subtype(subtype);
        let provenanced_constraint = ProvenancedConstraint::new(provenance, constraint);

        self.push_constraint(provenanced_constraint);
    }

    fn push_constraint(&mut self, constr: ProvenancedConstraint) {
        self.push_constraints(vec![constr]);
    }

    fn push_constraints(&mut self, mut queued: Vec<ProvenancedConstraint>) {
        while let Some(provenanced_constraint) = queued.pop() {
            match self.constraint_solver.solver.entail(&provenanced_constraint.constraint) {
                Ok(Step::Derived(constrs)) => {
                    queued.extend(constrs.into_iter().map(|x| ProvenancedConstraint {
                        provenance: provenanced_constraint.provenance.clone(),
                        constraint: x,
                    }));
                }

                Ok(Step::Subst(subst)) => {
                    self.move_constraints_from_residual(&subst, &mut queued);

                    self.constraint_solver.subst.compose(&subst, &self.engine);
                }

                Ok(Step::NoProgress) => {
                    if let Some(reduced_constraint) = provenanced_constraint.reduce(&self.engine) {
                        queued.push(reduced_constraint);
                    } else {
                        self.constraint_solver.residual_constraints.push(provenanced_constraint);
                    }
                }

                Err(err) => {
                    self.constraint_solver.errored_constraints.push((err, provenanced_constraint));
                }
            }
        }
    }

    fn move_constraints_from_residual(
        &mut self,
        subst: &Subst,
        queued: &mut Vec<ProvenancedConstraint>,
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
    #[must_use]
    pub fn residual_into_diags(mut self, engine: &TrackedEngine) -> (Vec<Diagnostic>, Subst) {
        for (_, provenanced_constraint) in &mut self.errored_constraints {
            provenanced_constraint.apply_in_place(&self.subst, engine);
        }

        let mut diags = self
            .residual_constraints
            .into_iter()
            .chain(self.errored_constraints.into_iter().map(|x| x.1))
            .filter_map(|x| match x.provenance {
                Provenance::Subtype(subtype_provenance) => Some(Diagnostic::ResidualSubtype(
                    ResidualSubtype::builder().provenance(subtype_provenance).build(),
                )),
                // TODO: Implement a correct diagnostic for effect composition errors. For now, we
                // just ignore them.
                Provenance::EffectCompose(_) => None,
            })
            .collect::<Vec<_>>();

        diags.dedup();

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

impl TAstBuilder {
    pub fn new_type_inference(&mut self) -> Interned<Ty> {
        self.new_type_inference_with_kind(TyKind::Star)
    }

    pub fn new_type_inference_with_kind(&mut self, kind: TyKind) -> Interned<Ty> {
        self.engine.intern(Ty::Inference(self.constraint_solver.solver.new_inference(kind)))
    }

    pub fn new_numeric_type_inference(&mut self) -> Interned<Ty> {
        let inference = self
            .constraint_solver
            .solver
            .new_inference_with_constraint(TyKind::Star, InferenceConstraint::Numeric);
        self.constraint_solver.numeric_inferences.push(inference);
        self.engine.intern(Ty::Inference(inference))
    }

    pub fn new_equality_comparable_type_inference(&mut self) -> Interned<Ty> {
        let inference = self
            .constraint_solver
            .solver
            .new_inference_with_constraint(TyKind::Star, InferenceConstraint::EqualityComparable);
        self.engine.intern(Ty::Inference(inference))
    }
}
