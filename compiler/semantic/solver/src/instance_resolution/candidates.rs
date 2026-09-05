use rayc_qbice::TrackedEngine;
use rayc_semantic_element::{
    all_instance_implements_trait::get_all_instance_implements_trait,
    instance_trait_ref::get_instance_trait_ref,
};
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    poly_var::{PolyVarID, get_poly_var_map},
    reduce::Reduce,
    solver::Solver,
    subst::Subst,
    trait_ref::TraitRef,
};

use super::{InstanceResolutionState, InstanceResolutionStateError};

/// A global instance whose head matches the requested trait reference.
#[derive(Debug)]
pub(super) struct InstanceCandidate {
    subst: Subst,
    instance_id: GlobalSymbolID,
    pending_given_parameters: Vec<PolyVarID>,
}

impl InstanceCandidate {
    #[must_use]
    pub(super) const fn instance_id(&self) -> GlobalSymbolID { self.instance_id }

    pub(super) fn into_parts(self) -> (Subst, GlobalSymbolID, Vec<PolyVarID>) {
        (self.subst, self.instance_id, self.pending_given_parameters)
    }
}

/// Collects globally eligible instances whose heads match the required trait.
pub(super) async fn collect(
    engine: &TrackedEngine,
    solver: &mut Solver,
    state: &mut InstanceResolutionState,
    site: GlobalSymbolID,
    required: &TraitRef,
) -> Result<Vec<InstanceCandidate>, InstanceResolutionStateError> {
    let instance_ids =
        engine.get_all_instance_implements_trait(required.trait_id(), site.target_id).await;

    let mut candidates = Vec::new();
    for instance_id in instance_ids.iter().copied() {
        let Some(head) = engine.get_instance_trait_ref(instance_id).await else {
            continue;
        };
        state.visit_candidate(instance_id)?;

        let head = head.normalize(engine);
        let Some(subst) = solver.head_match(&head, required).await else {
            continue;
        };
        let pending_given_parameters = engine
            .get_poly_var_map(instance_id)
            .await
            .iter()
            .filter_map(|(id, parameter)| parameter.trait_ref().is_some().then_some(id))
            .collect();
        candidates.push(InstanceCandidate { subst, instance_id, pending_given_parameters });
    }

    Ok(candidates)
}
