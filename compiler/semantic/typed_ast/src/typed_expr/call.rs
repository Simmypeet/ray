use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    subst::{MutSubstitutable, Subst, Substitutable},
    ty::Ty,
};

use crate::typed_expr::{SubExprs, TypedExprID};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
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
        /// [`trait_def_id`].
        trait_def_subst: Subst,
    },

    EffectOperation {
        effect_id: GlobalSymbolID,
        operation_id: GlobalSymbolID,
        subst: Subst,
    },

    Lambda {
        callee: TypedExprID,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Call {
    target: CallTarget,
    arguments: Vec<TypedExprID>,
}

impl Call {
    #[must_use]
    pub const fn new_direct(
        function_id: GlobalSymbolID,
        arguments: Vec<TypedExprID>,
        subst: Subst,
    ) -> Self {
        Self { target: CallTarget::Direct { function_id, subst }, arguments }
    }

    #[must_use]
    pub const fn new_unresolved_instance_associated(
        instance: Interned<Ty>,
        trait_def_id: GlobalSymbolID,
        trait_def_subst: Subst,
        arguments: Vec<TypedExprID>,
    ) -> Self {
        Self {
            target: CallTarget::UnresolvedInstanceAssociated {
                instance,
                trait_def_id,
                trait_def_subst,
            },
            arguments,
        }
    }

    #[must_use]
    pub const fn new_lambda(callee: TypedExprID, arguments: Vec<TypedExprID>) -> Self {
        Self { target: CallTarget::Lambda { callee }, arguments }
    }

    #[must_use]
    pub const fn new_effect_operation(
        effect_id: GlobalSymbolID,
        operation_id: GlobalSymbolID,
        arguments: Vec<TypedExprID>,
        subst: Subst,
    ) -> Self {
        Self { target: CallTarget::EffectOperation { effect_id, operation_id, subst }, arguments }
    }

    #[must_use]
    pub const fn target(&self) -> &CallTarget { &self.target }

    #[must_use]
    pub fn arguments(&self) -> &[TypedExprID] { &self.arguments }
}

impl MutSubstitutable for Call {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        match &mut self.target {
            CallTarget::Direct { subst: call_subst, .. }
            | CallTarget::EffectOperation { subst: call_subst, .. } => {
                call_subst.apply_mut_subst(subst, engine);
            }
            CallTarget::UnresolvedInstanceAssociated { instance, trait_def_subst, .. } => {
                instance.apply_in_place(subst, engine);
                trait_def_subst.apply_mut_subst(subst, engine);
            }
            CallTarget::Lambda { .. } => {}
        }
    }
}

impl SubExprs for Call {
    fn sub_exprs(&self) -> impl Iterator<Item = TypedExprID> {
        let target_iter = match &self.target {
            CallTarget::UnresolvedInstanceAssociated { .. }
            | CallTarget::Direct { .. }
            | CallTarget::EffectOperation { .. } => None,

            CallTarget::Lambda { callee } => Some(*callee),
        };

        target_iter.into_iter().chain(self.arguments.iter().copied())
    }
}
