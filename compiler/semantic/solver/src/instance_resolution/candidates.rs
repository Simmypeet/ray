use rayc_semantic_element::all_instance_implements_trait::get_all_instance_implements_trait;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    constraint::outlives::OutlivesConstraint,
    poly_var::{GlobalPolyVarID, PolyVarID, PolyVarMap, get_poly_var_map},
    subst::Subst,
    trait_ref::{TraitRef, get_instance_trait_ref},
};

use crate::{Solver, instance_resolution::InstanceResolutionError};

/// A global instance whose head matches the requested trait reference.
#[derive(Debug)]
pub(super) struct InstanceCandidate {
    subst: Subst,

    /// The outlives constraints of matching the head against the goal.
    outlives: Vec<OutlivesConstraint>,
    instance_id: GlobalSymbolID,
    pending_given_parameters: Vec<PolyVarID>,
}

impl InstanceCandidate {
    #[must_use]
    pub(super) const fn instance_id(&self) -> GlobalSymbolID { self.instance_id }

    pub(super) fn into_parts(
        self,
    ) -> (Subst, Vec<OutlivesConstraint>, GlobalSymbolID, Vec<PolyVarID>) {
        (self.subst, self.outlives, self.instance_id, self.pending_given_parameters)
    }
}

/// A given matched by the head already has its dictionary in `subst` and must
/// not be resolved again from the candidate's surrounding scope.
fn pending_given_parameters(
    parameters: &PolyVarMap,
    instance_id: GlobalSymbolID,
    subst: &Subst,
) -> Vec<PolyVarID> {
    parameters
        .iter()
        .filter_map(|(id, parameter)| {
            (parameter.trait_ref().is_some()
                && subst.get(&GlobalPolyVarID::new(instance_id, id)).is_none())
            .then_some(id)
        })
        .collect()
}

/// Matches the one source instance selected by a nominal Drop plan, without
/// consulting the global candidate index or spending ranking/search fuel.
pub(super) async fn selected(
    solver: &mut Solver,
    required: &TraitRef,
    instance_id: GlobalSymbolID,
) -> Option<InstanceCandidate> {
    let engine = solver.engine().clone();
    let head = engine.get_instance_trait_ref(instance_id).await?;
    let head = solver.normalize(&head).await;
    let (subst, outlives) = solver.head_match(&head, required).await?.into_parts();

    let parameters = engine.get_poly_var_map(instance_id).await;
    let pending_given_parameters = pending_given_parameters(&parameters, instance_id, &subst);

    Some(InstanceCandidate { subst, outlives, instance_id, pending_given_parameters })
}

/// Collects globally eligible instances whose heads match the required trait.
pub(super) async fn collect(
    solver: &mut Solver,
    required: &TraitRef,
) -> Result<Vec<InstanceCandidate>, InstanceResolutionError> {
    let engine = solver.engine().clone();
    let instance_ids = engine
        .get_all_instance_implements_trait(required.trait_id(), solver.site().target_id)
        .await;

    let mut candidates = Vec::new();
    for instance_id in instance_ids.iter().copied() {
        let Some(head) = engine.get_instance_trait_ref(instance_id).await else {
            continue;
        };
        solver.visit_instance_candidate(instance_id)?;

        // TODO: actually, we'd like for the instance-trait-ref to already be normalized
        // so that we can avoid this extra normalization step.
        let head = solver.normalize(&head).await;
        let Some(solution) = solver.head_match(&head, required).await else {
            continue;
        };
        let (subst, outlives) = solution.into_parts();

        let parameters = engine.get_poly_var_map(instance_id).await;
        let pending_given_parameters = pending_given_parameters(&parameters, instance_id, &subst);

        candidates.push(InstanceCandidate {
            subst,
            outlives,
            instance_id,
            pending_given_parameters,
        });
    }

    Ok(candidates)
}
