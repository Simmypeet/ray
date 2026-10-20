use qbice::storage::intern::Interned;
use rayc_qbice::TrackedEngine;

use crate::{
    constraint::outlives::OutlivesConstraints,
    reduce::Reduce,
    ty::{Ty, application::View as ApplicationView},
};

impl Reduce for Interned<Ty> {
    async fn reduce(
        &self,
        engine: &TrackedEngine,
        givens: &[crate::where_clause::PredicateKind],
    ) -> Option<(Self, OutlivesConstraints)> {
        // Prefer structural reduction, then the first matching given equality.
        if let Some(reduced) = reduce_type(self, engine, givens).await {
            return Some(reduced);
        }
        for equality in givens.iter().filter_map(crate::where_clause::PredicateKind::as_equality) {
            // Lifetimes never decide whether a given applies, just as they
            // never decide which instance is selected. Projection arguments
            // are invariant, so the lifetimes of a matching given are related
            // by equality.
            if equality.right() == self {
                continue;
            }
            if let Some(outlives) = Ty::equal_modulo_lifetimes(equality.left(), self, engine).await
            {
                return Some((equality.right().clone(), outlives));
            }
        }
        None
    }
}

async fn reduce_type(
    ty: &Interned<Ty>,
    engine: &TrackedEngine,
    givens: &[crate::where_clause::PredicateKind],
) -> Option<(Interned<Ty>, OutlivesConstraints)> {
    match ty.as_ref() {
        Ty::Application(application) => {
            Box::pin(async move {
                if let ApplicationView::InstanceAssociated(associated) = application.view()
                    && let Some(reduced) =
                        crate::reduce::reduce_instance_associated(associated, engine).await
                    && reduced != *ty
                {
                    return Some((reduced, OutlivesConstraints::new()));
                }
                if let ApplicationView::TraitDefCoroutine(coroutine) = application.view()
                    && let Some(reduced) =
                        crate::reduce::reduce_trait_def_coroutine(coroutine, engine).await
                {
                    return Some((reduced, OutlivesConstraints::new()));
                }
                application.reduce(engine, givens).await.map(|(application, outlives)| {
                    (engine.intern(Ty::Application(application)), outlives)
                })
            })
            .await
        }
        // A lifetime never reduces.
        Ty::Inference(_) | Ty::PolyVar(_) | Ty::SelfInstance(_) | Ty::Lifetime(_) => None,
        Ty::EffectRow(row) => {
            if row.labels().len() == 0
                && let Some(tail) = row.tail()
            {
                return Some((tail.clone(), OutlivesConstraints::new()));
            }
            Box::pin(row.reduce(engine, givens))
                .await
                .map(|(row, outlives)| (engine.intern(Ty::EffectRow(row)), outlives))
        }
    }
}
