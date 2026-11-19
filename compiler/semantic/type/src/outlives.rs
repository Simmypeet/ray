//! Decomposes outlives requirements `ty: 'a` into components (Rust RFC 1214).

use qbice::{Decode, Encode, Query, StableHash, storage::intern::Interned};
use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;

use crate::{
    ty::{
        Ty, TyKind,
        application::{Application, View as ApplicationView},
        lifetime::Lifetime,
    },
    where_clause::OutlivesPredicate,
};

/// Retrieves the outlives requirements implied by an ADT's references in its
/// fields (Rust RFC 2093), so users need not restate them.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<[OutlivesPredicate]>)]
#[extend(by_val, name = get_inferred_outlives)]
pub struct InferredOutlivesKey {
    /// A `SymbolKind::Strut` or `SymbolKind::Enum` symbol.
    pub adt_id: GlobalSymbolID,
}

/// A part of a type that an outlives requirement decomposes into.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OutlivesComponent {
    /// A lifetime parameter or a region variable. `'static` is never a
    /// component, because it outlives every lifetime.
    Region(Interned<Ty>),

    /// A type that cannot be decomposed further, so whether it outlives a
    /// lifetime can only come from an assumption.
    Opaque(Interned<Ty>),
}

impl OutlivesComponent {
    /// Returns the type this component stands for.
    #[must_use]
    pub const fn ty(&self) -> &Interned<Ty> {
        match self {
            Self::Region(ty) | Self::Opaque(ty) => ty,
        }
    }
}

impl Ty {
    /// Decomposes `ty: 'a` into the components of `ty` that must each outlive
    /// `'a`, in unspecified order.
    pub async fn outlives_components(
        ty: &Interned<Self>,
        engine: &TrackedEngine,
    ) -> Vec<OutlivesComponent> {
        // TODO: Let's decide do we really want `TyKind::EffectRow` and
        // `TyKind::Instance` to participate in outlives components. To be
        // conservative, we will include them for now, but we should revisit
        // this decision later.
        let mut components = Vec::new();
        let mut pending = vec![ty.clone()];

        while let Some(ty) = pending.pop() {
            match &*ty {
                Self::Lifetime(lifetime) => {
                    if lifetime.is_outlives_component() {
                        components.push(OutlivesComponent::Region(ty.clone()));
                    }
                }
                Self::PolyVar(_) => match ty.kind_of(engine).await {
                    TyKind::Lifetime => components.push(OutlivesComponent::Region(ty.clone())),
                    TyKind::Instance | TyKind::Star | TyKind::EffectRow | TyKind::EffectRow2 => {
                        components.push(OutlivesComponent::Opaque(ty.clone()));
                    }
                },
                Self::Application(application) => {
                    if application.is_outlives_projection() {
                        components.push(OutlivesComponent::Opaque(ty.clone()));
                    } else {
                        pending.extend(application.interned_iter().cloned());
                    }
                }
                Self::EffectRow(row) => {
                    pending.extend(row.interned_iter().cloned());
                }
                Self::EffectRow2(row) => {
                    pending.extend(row.interned_iter().cloned());
                }
                Self::SelfInstance(_) => components.push(OutlivesComponent::Opaque(ty.clone())),
                Self::Inference(_) => {}
            }
        }

        components
    }
}

impl Lifetime {
    /// Returns whether an outlives requirement on this lifetime needs to be
    /// checked at all.
    const fn is_outlives_component(self) -> bool {
        match self {
            // `'static` outlives everything, and erased lifetimes are checked
            // on the IR.
            Self::Static | Self::Erased => false,
            Self::Region(_) | Self::External(_) => true,
        }
    }
}

impl Application {
    /// Returns whether this application is a rigid projection, which is kept
    /// whole as an outlives component instead of being decomposed.
    fn is_outlives_projection(&self) -> bool {
        match self.view() {
            ApplicationView::InstanceAssociated(_) | ApplicationView::TraitDefCoroutine(_) => true,
            ApplicationView::Primitive(_)
            | ApplicationView::Tuple(_)
            | ApplicationView::Pointer(_)
            | ApplicationView::Reference(_)
            | ApplicationView::Struct(_)
            | ApplicationView::Enum(_)
            | ApplicationView::Instance(_)
            | ApplicationView::Closure(_)
            | ApplicationView::DefInstance(_)
            | ApplicationView::NoOpDropInstance(_)
            | ApplicationView::TupleDropInstance(_)
            | ApplicationView::ClosureDropInstance(_)
            | ApplicationView::DefCoroutine(_)
            | ApplicationView::ClosureCoroutine(_)
            | ApplicationView::ContinuationDefInstance(_)
            | ApplicationView::CoroutineInstance(_)
            | ApplicationView::CoroutineDropInstance(_)
            | ApplicationView::GeneratedAdtDropInstance(_)
            | ApplicationView::Error => false,
        }
    }
}
