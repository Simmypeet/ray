use qbice::storage::intern::Interned;
use rayc_type::{poly_var::get_enclosing_poly_var_maps, trait_ref::TraitRef, ty::Ty};

use super::InstanceResolutionError;
use crate::Solver;

pub(super) enum LexicalResolution {
    NotFound,
    Resolved(Interned<Ty>),
}

/// Resolves the nearest exact lexical dictionary before global search begins.
pub(super) async fn resolve(
    solver: &Solver,
    required: &TraitRef,
) -> Result<LexicalResolution, InstanceResolutionError> {
    let engine = solver.engine();
    let lexical_scope = engine.get_enclosing_poly_var_maps(solver.site()).await;
    let mut matching_scope = None;
    let mut candidates = Vec::new();

    // Normalize candidates through the solver so lexical matching observes the
    // same visible predicates as every other resolution step.
    for candidate_id in lexical_scope.all_poly_vars() {
        if matching_scope.is_some_and(|scope| scope != candidate_id.parent_id()) {
            break;
        }
        let Some(candidate) = lexical_scope.trait_ref_of(candidate_id) else {
            continue;
        };

        // TODO: let's avoid using syntactic equality here and instead use Solver's
        // exhaustive solving
        if solver.normalize(candidate).await == *required {
            matching_scope = Some(candidate_id.parent_id());
            candidates.push(candidate_id);
        }
    }

    match candidates.as_slice() {
        [] => Ok(LexicalResolution::NotFound),
        [candidate] => Ok(LexicalResolution::Resolved(Ty::new_poly_var(*candidate, engine))),
        [_, _, ..] => Err(InstanceResolutionError::AmbiguousLexical {
            required: required.clone(),
            candidates,
        }),
    }
}
