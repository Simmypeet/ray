//! Selection of the `Drop` dictionary which drops a value of a given type.

use qbice::storage::intern::Interned;
use rayc_lexical::tree::RelativeSpan;
use rayc_resolution::obligation::PredicateObligation;
use rayc_solver::{Solver, instance_resolution::InstanceResolutionError};
use rayc_symbol::{
    GlobalSymbolID,
    core_item::{CoreItem, get_core_item},
};
use rayc_type::{
    trait_ref::TraitRef,
    ty::{Ty, args::Args},
    where_clause::PredicateKind,
};

use crate::{Diagnostic, diagnostic::UnresolvedScopeDrop};

/// Resolves the `Drop` dictionary for `ty` in the solver's environment and
/// verifies the predicates required by the selected instances.
///
/// A type containing an error fails without a [`DropFailure`], since the
/// erroneous type was already reported where it was written.
pub async fn resolve_drop_instance(
    solver: &mut Solver,
    ty: Interned<Ty>,
) -> Result<Interned<Ty>, Vec<DropFailure>> {
    let engine = solver.engine();
    let drop_trait = engine.get_core_item(CoreItem::DropTrait).await;
    let trait_ref = TraitRef::new(drop_trait, Args::new([ty], engine));

    // The requirement is ground after type checking, so any failure is final
    // and the drop cannot be emitted.
    let (instance, obligations) = match solver.resolve_instance(trait_ref.clone()).await {
        Ok(resolved) => resolved.into_parts(),
        Err(InstanceResolutionError::ContainsError(_)) => return Err(Vec::new()),
        Err(error) => return Err(vec![DropFailure::Unresolved(trait_ref, error)]),
    };

    // Instance search returns the where-clause predicates of the selected
    // proof tree rather than checking them itself.
    let mut failures = Vec::new();
    for obligation in obligations {
        let (instance_id, predicate) = obligation.into_parts();
        if !solver.entails_predicate(&predicate).await {
            failures.push(DropFailure::UnsatisfiedPredicate(instance_id, predicate));
        }
    }

    if failures.is_empty() { Ok(instance) } else { Err(failures) }
}

/// Why no usable `Drop` dictionary exists for a type.
#[derive(Debug, Clone)]
pub enum DropFailure {
    /// No instance of `Drop` was found.
    Unresolved(TraitRef, InstanceResolutionError),

    /// A where-clause predicate of the selected instance does not hold.
    UnsatisfiedPredicate(GlobalSymbolID, PredicateKind),
}

impl DropFailure {
    /// Reports this failure against the value declared at `span`.
    #[must_use]
    pub fn into_diagnostic(self, span: RelativeSpan) -> Diagnostic {
        match self {
            Self::Unresolved(trait_ref, error) => {
                UnresolvedScopeDrop::new(span, trait_ref, error).into()
            }
            Self::UnsatisfiedPredicate(instance_id, predicate) => {
                Diagnostic::UnsatisfiedScopeDropPredicate(PredicateObligation::new(
                    predicate,
                    instance_id,
                    span,
                ))
            }
        }
    }
}
