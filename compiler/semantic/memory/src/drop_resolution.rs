//! Selection of the `Drop` dictionary which drops a value of a given type.

use qbice::storage::intern::Interned;
use rayc_lexical::tree::RelativeSpan;
use rayc_resolution::obligation::PredicateObligation;
use rayc_solver::{
    Solver,
    explain::{ProofFailure, RequiredBy, UnsatisfiedGoal},
};
use rayc_symbol::{
    GlobalSymbolID,
    core_item::{CoreItem, get_core_item},
};
use rayc_type::{
    trait_ref::TraitRef,
    ty::{Ty, args::Args},
    where_clause::PredicateKind,
};

use crate::Diagnostic;

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
    let trait_ref = TraitRef::new(drop_trait, Args::new([ty.clone()], engine));

    // The requirement is ground after type checking, so any failure is final
    // and the drop cannot be emitted.
    // Lifetimes are checked by the borrow checker, so the outlives
    // constraints of the selection are not needed here.
    let (instance, obligations, _) = match solver.resolve_instance(trait_ref.clone()).await {
        Ok(resolved) => resolved.into_parts(),
        Err(error) => {
            // A failure that involves the error type has no explanation,
            // since the error was already reported.
            return Err(ProofFailure::from_instance_error(trait_ref, &error)
                .map(|failure| DropFailure::Unresolved { dropped: ty, failure })
                .into_iter()
                .collect());
        }
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
    /// No `Drop` dictionary of `dropped` could be selected.
    Unresolved { dropped: Interned<Ty>, failure: ProofFailure },

    /// A where-clause predicate of the selected instance does not hold.
    UnsatisfiedPredicate(GlobalSymbolID, PredicateKind),
}

impl DropFailure {
    /// Reports this failure against the value declared at `span`.
    #[must_use]
    pub fn into_diagnostic(self, span: RelativeSpan) -> Diagnostic {
        self.into_diagnostic_at(span, None)
    }

    /// Reports this failure against the value declared at `span`, which is
    /// held across the suspension point at `suspension_span` and dropped if
    /// the function is abandoned there.
    #[must_use]
    pub fn into_suspension_diagnostic(
        self,
        span: RelativeSpan,
        suspension_span: RelativeSpan,
    ) -> Diagnostic {
        self.into_diagnostic_at(span, Some(suspension_span))
    }

    fn into_diagnostic_at(
        self,
        span: RelativeSpan,
        suspension_span: Option<RelativeSpan>,
    ) -> Diagnostic {
        match self {
            Self::Unresolved { dropped, failure } => {
                // The `Drop` dictionary of a coroutine is assembled from one
                // dictionary per argument, which the programmer acts on.
                let required_by = if dropped.as_coroutine_view().is_some() {
                    RequiredBy::CoroutineArguments { suspension: suspension_span }
                } else {
                    RequiredBy::ScopeDrop { suspension: suspension_span }
                };

                Diagnostic::UnresolvedScopeDrop(Box::new(UnsatisfiedGoal::new(
                    span,
                    required_by,
                    failure,
                )))
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
