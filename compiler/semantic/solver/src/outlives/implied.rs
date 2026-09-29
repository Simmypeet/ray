//! Outlives bounds implied by well-formedness.
//!
//! A reference `&'a t` is well-formed only when `t: 'a`, so a declaration may
//! assume that bound wherever such a reference appears in its signature. This
//! is the only source of implied bounds: unlike Rust, outlives predicates
//! declared in a where clause are never implied by the types that use them.

use std::collections::BTreeSet;

use qbice::storage::intern::Interned;
use rayc_hash::FxHashMap;
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::{parameter::get_parameter_map, return_type::get_return_type};
use rayc_symbol::{
    GlobalSymbolID,
    symbol_kind::{SymbolKind, get_symbol_kind},
};
use rayc_type::{
    outlives::get_inferred_outlives,
    subst::Substitutable,
    ty::{Ty, application::View as ApplicationView},
    where_clause::OutlivesPredicate,
};

/// Collects the outlives bounds that the references in a type imply.
///
/// - `&'a t` implies `t: 'a`;
/// - a struct `S[args]` implies its inferred outlives predicates, instantiated
///   with `args`, which come from the references among its fields.
///
/// Declared outlives predicates, such as those of a struct or an effect, are
/// never collected. Every other constructor only implies the bounds of its
/// arguments.
#[derive(Debug)]
pub struct ImpliedOutlivesCollector<'x> {
    engine: &'x TrackedEngine,

    /// Inferred outlives predicates that are still being computed, used in
    /// place of the query for the structs they contain.
    in_progress: Option<&'x FxHashMap<GlobalSymbolID, BTreeSet<OutlivesPredicate>>>,

    requirements: Vec<OutlivesPredicate>,
}

impl<'x> ImpliedOutlivesCollector<'x> {
    /// Creates a collector that reads inferred outlives predicates from the
    /// query.
    #[must_use]
    pub const fn new(engine: &'x TrackedEngine) -> Self {
        Self { engine, in_progress: None, requirements: Vec::new() }
    }

    /// Creates a collector for the inferred outlives fixed point. Structs in
    /// `in_progress` use its current sets instead of the query.
    #[must_use]
    pub const fn with_in_progress(
        engine: &'x TrackedEngine,
        in_progress: &'x FxHashMap<GlobalSymbolID, BTreeSet<OutlivesPredicate>>,
    ) -> Self {
        Self { engine, in_progress: Some(in_progress), requirements: Vec::new() }
    }

    /// Collects the bounds implied by `ty` and by every type nested in it.
    pub async fn collect(&mut self, ty: &Interned<Ty>) {
        let mut pending = vec![ty.clone()];

        while let Some(ty) = pending.pop() {
            match &*ty {
                Ty::Application(application) => {
                    self.collect_application(application.view()).await;
                    pending.extend(Ty::interned_arguments(&ty).cloned());
                }
                Ty::EffectRow(row) => pending.extend(row.interned_iter().cloned()),
                Ty::Inference(_) | Ty::PolyVar(_) | Ty::SelfInstance(_) | Ty::Lifetime(_) => {}
            }
        }
    }

    /// Returns the collected bounds.
    #[must_use]
    pub fn into_requirements(self) -> Vec<OutlivesPredicate> { self.requirements }

    /// Collects the bounds implied by one type constructor, excluding those of
    /// its arguments.
    async fn collect_application(&mut self, view: ApplicationView<'_>) {
        match view {
            ApplicationView::Reference(reference) => {
                self.requirements.push(OutlivesPredicate::new(
                    reference.pointee().clone(),
                    reference.lifetime().clone(),
                ));
            }
            ApplicationView::Struct(view) => {
                let subst = view.create_subst(self.engine).await;
                let inferred = match self.in_progress.and_then(|map| map.get(&view.symbol_id())) {
                    Some(inferred) => inferred.iter().cloned().collect::<Vec<_>>(),
                    None => self.engine.get_inferred_outlives(view.symbol_id()).await.to_vec(),
                };
                self.requirements.extend(
                    inferred
                        .iter()
                        .map(|predicate| predicate.apply_subst_or_clone(&subst, self.engine)),
                );
            }
            ApplicationView::Primitive(_)
            | ApplicationView::Tuple(_)
            | ApplicationView::Pointer(_)
            | ApplicationView::Instance(_)
            | ApplicationView::InstanceAssociated(_)
            | ApplicationView::Closure(_)
            | ApplicationView::DefInstance(_)
            | ApplicationView::NoOpDropInstance(_)
            | ApplicationView::TupleDropInstance(_)
            | ApplicationView::ClosureDropInstance(_)
            | ApplicationView::NominalDropInstance(_)
            | ApplicationView::Error => {}
        }
    }
}

/// Returns the outlives bounds implied by the well-formedness of one
/// declaration, excluding its enclosing declarations:
///
/// - a plain `def` assumes the bounds implied by the references in its
///   parameter and return types;
/// - a struct assumes its inferred outlives predicates.
///
/// These become part of the symbol's
/// [`WhereClause`](rayc_type::where_clause::WhereClause). A marker
/// implementation also assumes the requirements of its head, which come from
/// resolving it and are added to its where clause separately. No other
/// declaration has implied bounds; its bounds must be written in its where
/// clause.
pub async fn implied_bounds(
    symbol_id: GlobalSymbolID,
    engine: &TrackedEngine,
) -> Vec<OutlivesPredicate> {
    match engine.get_symbol_kind(symbol_id).await {
        SymbolKind::Def => {
            let mut collector = ImpliedOutlivesCollector::new(engine);
            let parameters = engine.get_parameter_map(symbol_id).await;
            for (_, parameter) in parameters.iter() {
                collector.collect(parameter.ty()).await;
            }
            collector.collect(&engine.get_return_type(symbol_id).await).await;
            collector.into_requirements()
        }
        SymbolKind::Strut => engine.get_inferred_outlives(symbol_id).await.to_vec(),
        SymbolKind::Effect
        | SymbolKind::EffectOperation
        | SymbolKind::ExternDef
        | SymbolKind::Instance
        | SymbolKind::InstanceDef
        | SymbolKind::InstanceType
        | SymbolKind::Marker
        | SymbolKind::MarkerImplementation
        | SymbolKind::Module
        | SymbolKind::Trait
        | SymbolKind::TraitDef
        | SymbolKind::TraitType => Vec::new(),
    }
}
