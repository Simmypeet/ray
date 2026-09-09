use qbice::storage::intern::Interned;
use rayc_semantic_element::instance_trait_ref::get_instance_trait_ref;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{trait_ref::TraitRef, ty::Ty};

use super::InstanceResolutionError;
use crate::Solver;

#[derive(Debug)]
pub(super) struct ViableInstance {
    instance_id: GlobalSymbolID,
    term: Interned<Ty>,
}

impl ViableInstance {
    #[must_use]
    pub(super) const fn new(instance_id: GlobalSymbolID, term: Interned<Ty>) -> Self {
        Self { instance_id, term }
    }
}

/// Selects the unique maximal viable candidate under the specificity relation.
pub(super) async fn select(
    solver: &mut Solver,
    required: &TraitRef,
    candidates: &[ViableInstance],
) -> Result<Interned<Ty>, InstanceResolutionError> {
    let engine = solver.engine().clone();
    let mut heads = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let head = engine
            .get_instance_trait_ref(candidate.instance_id)
            .await
            .expect("a viable instance candidate must have a trait reference");
        let head = solver.normalize(&head).await;
        heads.push(head);
    }

    let maxima = maximal_candidates(solver, &heads).await;
    match maxima.as_slice() {
        [winner] => Ok(candidates[*winner].term.clone()),
        [] => unreachable!("a non-empty finite partial order has a maximal element"),
        [_, _, ..] => {
            let mut candidates =
                maxima.iter().map(|index| candidates[*index].instance_id).collect::<Vec<_>>();
            candidates.sort_unstable();
            Err(InstanceResolutionError::AmbiguousGlobal { required: required.clone(), candidates })
        }
    }
}

async fn maximal_candidates(solver: &mut Solver, heads: &[TraitRef]) -> Vec<usize> {
    let mut maxima = Vec::new();
    for (index, head) in heads.iter().enumerate() {
        let mut dominated = false;
        for (other_index, other) in heads.iter().enumerate() {
            if index != other_index && is_more_specific(solver, other, head).await {
                dominated = true;
                break;
            }
        }
        if !dominated {
            maxima.push(index);
        }
    }
    maxima
}

async fn is_more_specific(solver: &mut Solver, a: &TraitRef, b: &TraitRef) -> bool {
    let b_matches_a = solver.head_match(b, a).await.is_some();
    let a_matches_b = solver.head_match(a, b).await.is_some();
    b_matches_a && !a_matches_b
}
