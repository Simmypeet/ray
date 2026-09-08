use qbice::storage::intern::Interned;
use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{poly_var::get_enclosing_poly_var_maps, trait_ref::TraitRef, ty::Ty};

use super::InstanceResolutionError;

pub(super) enum LexicalResolution {
    NotFound,
    Resolved(Interned<Ty>),
}

/// Resolves the nearest exact lexical dictionary before global search begins.
pub(super) async fn resolve(
    engine: &TrackedEngine,
    site: GlobalSymbolID,
    required: &TraitRef,
) -> Result<LexicalResolution, InstanceResolutionError> {
    let lexical_scope = engine.get_enclosing_poly_var_maps(site).await;
    let candidates = lexical_scope.nearest_instance_matches(required, engine).await;
    match candidates.as_slice() {
        [] => Ok(LexicalResolution::NotFound),
        [candidate] => Ok(LexicalResolution::Resolved(Ty::new_poly_var(*candidate, engine))),
        [_, _, ..] => Err(InstanceResolutionError::AmbiguousLexical {
            required: required.clone(),
            candidates,
        }),
    }
}
