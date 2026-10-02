use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    poly_var::build_subst_from_args,
    subst::Subst,
    ty::{Ty, self_instance::SelfInstance},
};

use crate::{
    ir_expr::IRExprID,
    visit::{
        TypeSite, TypeVisitor, TypeVisitorMut, TypeVisitorMutAsync, VisitType, VisitTypeMut,
        VisitTypeMutAsync,
    },
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub enum CallTarget {
    Direct {
        function_id: GlobalSymbolID,
        subst: Subst,
    },

    /// Calls a method on an unresolved trait instance (e.g., `i.foo()` where
    /// `i` is an instance parameter), which is differ from a direct call to an
    /// instance associated method (e.g., `someInstanceSym.foo()` where
    /// `someInstanceSym` is a concrete instance symbol).
    UnresolvedInstanceAssociated {
        /// The unresolved instance term
        instance: Interned<Ty>,

        /// The **abstract** [`rayc_symbol::symbol_kind::SymbolKind::TraitDef`]
        /// symbol of the trait that defines the method being called.
        ///
        /// Since the trait def is abstract (has no definition), during
        /// monomorphization, once the concrete instance dictionary is resolved,
        trait_def_id: GlobalSymbolID,

        /// The substitution containing all the type parameters of the
        /// [`trait_def_id`] (doesn't include its enclosing parent trait).
        trait_def_subst: Subst,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct Call {
    target: CallTarget,
    arguments: Vec<IRExprID>,
    effect: Interned<Ty>,
}

impl CallTarget {
    /// Returns the symbol whose signature the call is checked against: the
    /// called function, or the abstract trait def of an unresolved instance.
    #[must_use]
    pub const fn signature_id(&self) -> GlobalSymbolID {
        match self {
            Self::Direct { function_id, .. } => *function_id,
            Self::UnresolvedInstanceAssociated { trait_def_id, .. } => *trait_def_id,
        }
    }

    /// Returns the dictionary the call dispatches through: the instance of
    /// an unresolved instance call, or `None` for a direct call.
    #[must_use]
    pub const fn dispatch_instance(&self) -> Option<&Interned<Ty>> {
        match self {
            Self::Direct { .. } => None,
            Self::UnresolvedInstanceAssociated { instance, .. } => Some(instance),
        }
    }

    /// Returns the substitution that instantiates the signature of
    /// [`Self::signature_id`] at the call.
    ///
    /// For an unresolved instance, the signature of the trait def also
    /// mentions the parameters of its enclosing trait and the trait's self
    /// dictionary. Those are instantiated with the arguments of the trait
    /// reference that the instance implements, and with the instance itself.
    pub async fn signature_subst(&self, engine: &TrackedEngine) -> Subst {
        match self {
            Self::Direct { subst, .. } => subst.clone(),

            Self::UnresolvedInstanceAssociated { instance, trait_def_subst, .. } => {
                let mut subst = trait_def_subst.clone();

                // An instance without a trait reference is recovery from an
                // invalid declaration, which was reported already.
                let Ok(trait_ref) = instance.instance_trait_ref(engine).await else {
                    return subst;
                };
                let trait_id = trait_ref.trait_id();

                // The mappings already chosen at the call take precedence.
                // The trait reference's arguments are types of the caller,
                // so they are added as they are, not composed.
                let trait_subst =
                    engine.build_subst_from_args(trait_id, trait_ref.args().interned_iter()).await;
                for (poly_var_id, argument) in trait_subst.poly_var_mappings() {
                    if subst.get(&poly_var_id).is_none() {
                        subst.insert(poly_var_id, argument.clone());
                    }
                }

                let self_instance = SelfInstance::new(trait_id);
                if subst.get(&self_instance).is_none() {
                    subst.insert(self_instance, instance.clone());
                }

                subst
            }
        }
    }
}

impl VisitType for Call {
    fn visit_types<V: TypeVisitor>(&self, site: TypeSite, visitor: &mut V) {
        match &self.target {
            CallTarget::Direct { subst, .. } => {
                for ty in subst.codomain() {
                    visitor.visit_type(ty, site);
                }
            }

            CallTarget::UnresolvedInstanceAssociated { instance, trait_def_subst, .. } => {
                visitor.visit_type(instance, site);
                for ty in trait_def_subst.codomain() {
                    visitor.visit_type(ty, site);
                }
            }
        }
        visitor.visit_type(&self.effect, site);
    }
}

impl VisitTypeMut for Call {
    fn visit_types_mut<V: TypeVisitorMut>(&mut self, site: TypeSite, visitor: &mut V) {
        match &mut self.target {
            CallTarget::Direct { subst, .. } => {
                for ty in subst.codomain_mut() {
                    visitor.visit_type_mut(ty, site);
                }
            }

            CallTarget::UnresolvedInstanceAssociated { instance, trait_def_subst, .. } => {
                visitor.visit_type_mut(instance, site);
                for ty in trait_def_subst.codomain_mut() {
                    visitor.visit_type_mut(ty, site);
                }
            }
        }
        visitor.visit_type_mut(&mut self.effect, site);
    }
}

impl VisitTypeMutAsync for Call {
    async fn visit_types_mut_async<V: TypeVisitorMutAsync>(
        &mut self,
        site: TypeSite,
        visitor: &mut V,
    ) {
        match &mut self.target {
            CallTarget::Direct { subst, .. } => {
                for ty in subst.codomain_mut() {
                    visitor.visit_type_mut_async(ty, site).await;
                }
            }

            CallTarget::UnresolvedInstanceAssociated { instance, trait_def_subst, .. } => {
                visitor.visit_type_mut_async(instance, site).await;
                for ty in trait_def_subst.codomain_mut() {
                    visitor.visit_type_mut_async(ty, site).await;
                }
            }
        }
        visitor.visit_type_mut_async(&mut self.effect, site).await;
    }
}

impl Call {
    #[must_use]
    pub const fn new_direct(
        function_id: GlobalSymbolID,
        arguments: Vec<IRExprID>,
        subst: Subst,
        effect: Interned<Ty>,
    ) -> Self {
        Self { target: CallTarget::Direct { function_id, subst }, arguments, effect }
    }

    #[must_use]
    pub const fn new_unresolved_instance_associated(
        instance: Interned<Ty>,
        trait_def_id: GlobalSymbolID,
        trait_def_subst: Subst,
        arguments: Vec<IRExprID>,
        effect: Interned<Ty>,
    ) -> Self {
        Self {
            target: CallTarget::UnresolvedInstanceAssociated {
                instance,
                trait_def_id,
                trait_def_subst,
            },
            arguments,
            effect,
        }
    }

    #[must_use]
    pub const fn target(&self) -> &CallTarget { &self.target }

    #[must_use]
    pub fn arguments(&self) -> &[IRExprID] { &self.arguments }

    /// Returns the conservative ambient effect row at this invocation site.
    #[must_use]
    pub const fn effect(&self) -> &Interned<Ty> { &self.effect }
}
