use qbice::{Decode, Encode, Identifiable, Query, StableHash, storage::intern::Interned};
use rayc_qbice::TrackedEngine;
use rayc_symbol::{
    GlobalSymbolID,
    core_item::{CoreItem, get_core_item},
};

use crate::{
    poly_var::{build_subst_from_args, get_poly_var_map},
    reduce::Reduce,
    subst::{Subst, Substitutable},
    ty::{Ty, application::View as ApplicationView, args::Args},
};

/// A reference to a trait together with its type arguments.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct TraitRef {
    trait_id: GlobalSymbolID,
    args: Args,
}

impl TraitRef {
    #[must_use]
    pub const fn new(trait_id: GlobalSymbolID, args: Args) -> Self { Self { trait_id, args } }

    #[must_use]
    pub const fn trait_id(&self) -> GlobalSymbolID { self.trait_id }

    #[must_use]
    pub const fn args(&self) -> &Args { &self.args }

    /// Whether any argument recursively contains an inference variable.
    #[must_use]
    pub fn contains_inference(&self) -> bool { self.args.contains_inference() }

    /// Whether any argument recursively contains an error type.
    #[must_use]
    pub fn contains_error(&self) -> bool { self.args.contains_error() }
}

/// Retrieves the trait reference represented by an instance symbol. The value
/// is absent when the trait reference cannot be resolved.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Option<TraitRef>)]
#[extend(by_val, name = get_instance_trait_ref)]
pub struct InstanceTraitRefKey {
    /// A `SymbolKind::Instance` symbol.
    pub symbol_id: GlobalSymbolID,
}

/// Why a type has no known trait reference as an instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InstanceTraitRefError {
    /// The type is an inference variable, which may still be solved to an
    /// instance.
    Inference,

    /// The type is not an instance.
    NotInstance,

    /// The type is an instance whose trait reference cannot be resolved,
    /// which is recovery from an invalid instance declaration.
    Unresolved,
}

impl Ty {
    /// Returns the trait reference that this instance implements: the
    /// declared trait of a dictionary variable, the enclosing trait of
    /// `this`, the instantiated head of a named instance, or the trait of a
    /// compiler-provided instance.
    pub async fn instance_trait_ref(
        &self,
        engine: &TrackedEngine,
    ) -> Result<TraitRef, InstanceTraitRefError> {
        let core_trait_ref = async |item, arg: &Interned<Self>| {
            TraitRef::new(engine.get_core_item(item).await, Args::new([arg.clone()], engine))
        };

        match self {
            Self::Inference(_) => Err(InstanceTraitRefError::Inference),
            Self::SelfInstance(instance) => Ok(instance.trait_ref(engine).await),
            Self::PolyVar(id) => engine
                .get_poly_var_map(id.parent_id())
                .await
                .trait_ref_of(id.id())
                .cloned()
                .ok_or(InstanceTraitRefError::Unresolved),
            Self::Application(application) => match application.view() {
                ApplicationView::DefInstance(closure) => {
                    Ok(core_trait_ref(CoreItem::DefTrait, closure).await)
                }
                ApplicationView::Instance(instance) => {
                    let head = engine
                        .get_instance_trait_ref(instance.symbol_id())
                        .await
                        .ok_or(InstanceTraitRefError::Unresolved)?;
                    let subst =
                        engine.build_subst_from_args(instance.symbol_id(), instance.args()).await;
                    Ok(head.apply_subst_or_clone(&subst, engine))
                }
                ApplicationView::NoOpDropInstance(no_op) => {
                    Ok(core_trait_ref(CoreItem::DropTrait, no_op).await)
                }
                ApplicationView::TupleDropInstance(instance) => {
                    Ok(core_trait_ref(CoreItem::DropTrait, instance.tuple()).await)
                }
                ApplicationView::ClosureDropInstance(instance) => {
                    Ok(core_trait_ref(CoreItem::DropTrait, instance.closure()).await)
                }
                ApplicationView::NominalDropInstance(instance) => {
                    Ok(core_trait_ref(CoreItem::DropTrait, instance.nominal()).await)
                }
                ApplicationView::Primitive(_)
                | ApplicationView::Tuple(_)
                | ApplicationView::Pointer(_)
                | ApplicationView::Reference(_)
                | ApplicationView::Struct(_)
                | ApplicationView::InstanceAssociated(_)
                | ApplicationView::Closure(_)
                | ApplicationView::Error => Err(InstanceTraitRefError::NotInstance),
            },
            Self::EffectRow(_) | Self::Lifetime(_) => Err(InstanceTraitRefError::NotInstance),
        }
    }
}

impl Reduce for TraitRef {
    async fn reduce(
        &self,
        engine: &rayc_qbice::TrackedEngine,
        givens: &[crate::where_clause::PredicateKind],
    ) -> Option<Self> {
        self.args.reduce(engine, givens).await.map(|args| Self::new(self.trait_id, args))
    }
}

impl Substitutable for TraitRef {
    fn apply_subst(&self, subst: &Subst, engine: &rayc_qbice::TrackedEngine) -> Option<Self> {
        self.args.apply_subst(subst, engine).map(|args| Self::new(self.trait_id, args))
    }
}
