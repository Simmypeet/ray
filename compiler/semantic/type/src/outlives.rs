//! The components of an outlives requirement `ty: 'a`.
//!
//! Following Rust RFC 1214, `ty: 'a` holds exactly when every component of
//! `ty` outlives `'a`. A component is a lifetime, a polymorphic variable of any
//! other kind (a type, an effect row, or a dictionary), or a rigid
//! associated-type projection. Every other type constructor is transparent: it
//! outlives `'a` when its arguments do.

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

/// Retrieves the outlives requirements inferred for a struct from its field
/// types, as in Rust RFC 2093.
///
/// For `struct Ref['a, t]: value: &'a t` this is `t: 'a`. Only references
/// are a source: the declared outlives predicates of a field's struct are not
/// inferred. The requirements are stated over the struct's own polymorphic
/// variables and hold for every well-formed use of the struct, next to its
/// declared where clause.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<[OutlivesPredicate]>)]
#[extend(by_val, name = get_inferred_outlives)]
pub struct InferredOutlivesKey {
    /// A `SymbolKind::Strut` symbol.
    pub struct_id: GlobalSymbolID,
}

/// A part of a type that an outlives requirement decomposes into.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OutlivesComponent {
    /// A lifetime parameter or a region variable. `'static` is never a
    /// component, because it outlives every lifetime.
    Region(Interned<Ty>),

    /// A polymorphic variable of kind `Star`, `EffectRow` or `Instance`, or
    /// the rigid `this` dictionary. Whether it outlives a lifetime can only
    /// come from an assumption.
    Param(Interned<Ty>),

    /// A rigid associated-type projection. It outlives a lifetime when an
    /// assumption says so, or when everything it projects from does.
    Projection(Interned<Ty>),
}

impl OutlivesComponent {
    /// Returns the type this component stands for.
    #[must_use]
    pub const fn ty(&self) -> &Interned<Ty> {
        match self {
            Self::Region(ty) | Self::Param(ty) | Self::Projection(ty) => ty,
        }
    }
}

impl Ty {
    /// Decomposes the requirement `ty: 'a` into the components of `ty` that
    /// must each outlive `'a`. The order of the components is unspecified.
    ///
    /// Erased lifetimes, errors and inference variables yield no component:
    /// the first are checked on the IR, and the others either were already
    /// reported or cannot occur where named lifetimes are checked.
    pub async fn outlives_components(
        ty: &Interned<Self>,
        engine: &TrackedEngine,
    ) -> Vec<OutlivesComponent> {
        // TODO: Let's decide do we really want `TyKind::EffectRow` and
        // `TyKind::Instance` to participate in outlives components. To be
        // conservative, we will include them for now, but we should revisit this
        // decision later.
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
                    TyKind::Instance | TyKind::Star | TyKind::EffectRow => {
                        components.push(OutlivesComponent::Param(ty.clone()));
                    }
                },
                Self::Application(application) => {
                    if application.is_outlives_projection() {
                        components.push(OutlivesComponent::Projection(ty.clone()));
                    } else {
                        pending.extend(application.interned_iter().cloned());
                    }
                }
                Self::EffectRow(row) => {
                    pending.extend(row.interned_iter().cloned());
                }
                Self::SelfInstance(_) => components.push(OutlivesComponent::Param(ty.clone())),
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
            ApplicationView::InstanceAssociated(_) => true,
            ApplicationView::Primitive(_)
            | ApplicationView::Tuple(_)
            | ApplicationView::Pointer(_)
            | ApplicationView::Reference(_)
            | ApplicationView::Struct(_)
            | ApplicationView::Instance(_)
            | ApplicationView::Closure(_)
            | ApplicationView::DefInstance(_)
            | ApplicationView::NoOpDropInstance(_)
            | ApplicationView::TupleDropInstance(_)
            | ApplicationView::ClosureDropInstance(_)
            | ApplicationView::NominalDropInstance(_)
            | ApplicationView::Error => false,
        }
    }
}
