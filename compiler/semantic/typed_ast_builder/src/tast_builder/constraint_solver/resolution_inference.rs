use rayc_lexical::tree::RelativeSpan;
use rayc_resolution::GenInferWithSpan;
use rayc_type::{
    trait_ref::TraitRef,
    ty::{
        InferenceConstraint, Ty, TyKind,
        inference::{GenInfer, Inference},
    },
};

use super::{
    ConstraintSolver, constraints::Constraint, provenance::RootCauseOrigin,
    solve::PendingConstraint,
};

/// Collects obligations for one syntactic resolution before asynchronous
/// solving.
pub(in crate::tast_builder) struct ResolutionInference<'a> {
    solver: &'a mut ConstraintSolver,
    constraints: Vec<PendingConstraint>,
}

impl<'a> ResolutionInference<'a> {
    pub(in crate::tast_builder) const fn new(solver: &'a mut ConstraintSolver) -> Self {
        Self { solver, constraints: Vec::new() }
    }

    pub(in crate::tast_builder) fn into_constraints(self) -> Vec<PendingConstraint> {
        self.constraints
    }
}

impl GenInferWithSpan for ResolutionInference<'_> {
    fn gen_infer(
        &mut self,
        kind: TyKind,
        constraint: InferenceConstraint,
        _span: RelativeSpan,
    ) -> Inference {
        self.solver.gen_infer(kind, constraint)
    }

    fn gen_instance_infer(
        &mut self,
        expected_trait_ref: &TraitRef,
        span: RelativeSpan,
    ) -> Inference {
        let inference = self.solver.gen_infer(TyKind::Instance, InferenceConstraint::Any);
        let cause_id = self.solver.provenance.insert_root_cause(RootCauseOrigin::InstanceResolve {
            trait_ref: expected_trait_ref.clone(),
            span,
        });

        // The caller submits these obligations after releasing the resolver's borrow.
        self.constraints.push(
            PendingConstraint::builder()
                .constraint(Constraint::InstanceResolve {
                    instance: self.solver.solver.engine().intern(Ty::Inference(inference)),
                    trait_ref: expected_trait_ref.clone(),
                })
                .cause_id(cause_id)
                .build(),
        );
        inference
    }
}
