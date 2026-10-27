//! Selection of the `Drop` dictionary which drops a value of a given type.

use qbice::storage::intern::Interned;
use rayc_handler::Handler2;
use rayc_lexical::tree::RelativeSpan;
use rayc_semantic_element::deferred_marker::DeferredMarker;
use rayc_solver::{
    Solver,
    explain::{ProofFailure, RequiredBy, UnsatisfiedPredicate},
    solver::MarkerEntailment,
};
use rayc_symbol::{
    GlobalSymbolID,
    core_item::{CoreItem, get_core_item},
};
use rayc_type::{
    trait_ref::TraitRef,
    ty::{Ty, args::Args},
    where_clause::{MarkerPredicate, PredicateKind},
};

use crate::Diagnostic;

/// A selected `Drop` dictionary, together with the marker predicates of the
/// selected instances which depend on what a coroutine holds.
#[derive(Debug, Clone)]
pub struct DropInstance {
    instance: Interned<Ty>,

    /// The marker predicates which the solver deferred, each with the
    /// instance whose where clause requires it.
    deferred: Vec<(GlobalSymbolID, MarkerPredicate)>,
}

impl DropInstance {
    /// Returns the selected `Drop` dictionary.
    #[must_use]
    pub const fn instance(&self) -> &Interned<Ty> { &self.instance }

    /// Returns the deferred marker predicates of this dictionary, attributed
    /// to the value declared at `span`.
    pub fn deferred_markers(&self, span: RelativeSpan) -> impl Iterator<Item = DeferredMarker> {
        self.deferred.iter().map(move |(instance_id, predicate)| {
            DeferredMarker::new(predicate.clone(), span, *instance_id)
        })
    }
}

/// The value whose drop requires a `Drop` dictionary, which a failure is
/// reported against.
#[derive(Debug, Clone, Copy)]
pub struct DroppedValue {
    /// The declaration of the dropped binding, or the dropped expression.
    pub span: RelativeSpan,

    /// The suspension point the value is held across, when it is dropped
    /// only because the function is abandoned there.
    pub suspension: Option<RelativeSpan>,
}

impl DroppedValue {
    #[must_use]
    pub const fn new_simple(span: RelativeSpan) -> Self { Self { span, suspension: None } }
}

/// Resolves the `Drop` dictionary for `ty` in the solver's environment and
/// verifies the predicates required by the selected instances.
///
/// Returns `None` when no usable dictionary exists, after reporting why
/// against `dropped` into `diagnostics`. A type containing an error is not
/// reported, since the erroneous type was already reported where it was
/// written.
///
/// A marker predicate which depends on what a coroutine holds, while the
/// solver cannot see it, does not fail the resolution. It is returned with
/// the dictionary, to be proved once the coroutine's saved locals are known.
pub async fn resolve_drop_instance(
    solver: &mut Solver,
    ty: Interned<Ty>,
    dropped: DroppedValue,
    diagnostics: &mut impl Handler2<Diagnostic>,
) -> Option<DropInstance> {
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
            if let Some(failure) = ProofFailure::from_instance_error(trait_ref, &error) {
                // The `Drop` dictionary of a coroutine is assembled from one
                // dictionary per argument, which the programmer acts on.
                let required_by = if ty.as_coroutine_view().is_some() {
                    RequiredBy::CoroutineArguments { suspension: dropped.suspension }
                } else {
                    RequiredBy::ScopeDrop { suspension: dropped.suspension }
                };

                diagnostics.receive(Diagnostic::UnsatisfiedDropPredicate(
                    UnsatisfiedPredicate::new_explained(dropped.span, required_by, failure),
                ));
            }

            return None;
        }
    };

    // Instance search returns the where-clause predicates of the selected
    // proof tree rather than checking them itself.
    let mut holds_all = true;
    let mut deferred = Vec::new();
    for obligation in obligations {
        let (instance_id, predicate) = obligation.into_parts();
        let holds = match &predicate {
            PredicateKind::Marker(marker) => {
                match solver.entails_marker_predicate(marker.clone()).await {
                    MarkerEntailment::Holds => true,
                    MarkerEntailment::Deferred => {
                        deferred.push((instance_id, marker.clone()));
                        true
                    }
                    MarkerEntailment::Fails | MarkerEntailment::Overflow => false,
                }
            }
            PredicateKind::AssociatedTypeEquality(_) | PredicateKind::Outlives(_) => {
                solver.entails_predicate(&predicate).await
            }
        };

        if holds {
            continue;
        }

        holds_all = false;
        let unsatisfied =
            solver.explain_unsatisfied_predicate(predicate, instance_id, dropped.span).await;
        diagnostics.receive(Diagnostic::UnsatisfiedDropPredicate(unsatisfied));
    }

    holds_all.then_some(DropInstance { instance, deferred })
}
