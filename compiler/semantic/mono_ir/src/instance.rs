use qbice::{Decode, Encode, Identifiable, StableHash};
use rayc_qbice::TrackedEngine;
use rayc_solver::Solver;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    poly_var::get_enclosing_poly_var_maps,
    subst::{Subst, Substitutable},
    ty::{
        Ty,
        application::{ClosureID, ClosureView},
    },
};

use crate::function::MonoFunctionID;

/// Identifies one concrete instantiation of a source definition.
///
/// This is intentionally shaped like the key of the future incremental `MonoIR`
/// query. Global references stored in one fragment therefore also describe the
/// other fragments required by the eventual program orchestrator.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct MonoDefInstance {
    def_id: GlobalSymbolID,
    substitution: Subst,
}

impl MonoDefInstance {
    /// Retains only the owner's semantic arguments, with normalized concrete
    /// values. Trait `SelfInstance` binders have already been replaced by
    /// `InstanceMember` correspondence before instance-method IR is built;
    /// they are not additional owner arguments.
    #[must_use]
    pub async fn new(def_id: GlobalSymbolID, arguments: Subst, engine: &TrackedEngine) -> Self {
        // Empty keys already have no caller bindings or arguments to normalize.
        if arguments.is_empty() {
            return Self { def_id, substitution: arguments };
        }
        let maps = engine.get_enclosing_poly_var_maps(def_id).await;
        let mut substitution = Subst::new_empty();
        for variable in maps.all_poly_vars() {
            let value =
                engine.intern(Ty::PolyVar(variable)).apply_subst_or_clone(&arguments, engine);
            let value = Solver::without_givens(engine.clone()).normalize(&value).await;
            assert!(
                value.recursive_iter().all(|ty| match ty {
                    Ty::Application(_) | Ty::EffectRow(_) => true,
                    Ty::PolyVar(_) | Ty::Inference(_) | Ty::SelfInstance(_) => false,
                }),
                "definition arguments must be concrete: {value:?}"
            );
            substitution.insert(variable, value);
        }
        Self { def_id, substitution }
    }

    #[must_use]
    pub const fn def_id(&self) -> GlobalSymbolID { self.def_id }

    #[must_use]
    pub const fn substitution(&self) -> &Subst { &self.substitution }
}

/// Identifies one concrete instantiation of an effect declaration.
///
/// It can also be used to represent an effect type that packages up its
/// operation handlers.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct MonoEffectInstance {
    effect_id: GlobalSymbolID,
    substitution: Subst,
}

impl MonoEffectInstance {
    #[must_use]
    pub const fn new(effect_id: GlobalSymbolID, substitution: Subst) -> Self {
        Self { effect_id, substitution }
    }

    #[must_use]
    pub const fn effect_id(&self) -> GlobalSymbolID { self.effect_id }

    #[must_use]
    pub const fn substitution(&self) -> &Subst { &self.substitution }
}

/// References either a nested function in the current fragment or the root
/// function of another concrete definition fragment.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum FunctionReference {
    Local(MonoFunctionID),
    Global(MonoDefInstance),
    Closure(MonoClosureInstance),
}

/// A source closure in one concrete owner specialization.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct MonoClosureInstance {
    owner: MonoDefInstance,
    closure_id: ClosureID,
}

impl MonoClosureInstance {
    #[must_use]
    pub const fn new(owner: MonoDefInstance, closure_id: ClosureID) -> Self {
        Self { owner, closure_id }
    }

    #[must_use]
    pub const fn owner(&self) -> &MonoDefInstance { &self.owner }

    pub async fn from_closure(engine: &TrackedEngine, closure: ClosureView<'_>) -> Self {
        let maps = engine.get_enclosing_poly_var_maps(closure.owner_id()).await;

        let variables = maps.all_poly_vars();
        let substitution =
            variables.zip(closure.owner_arguments().iter().cloned()).collect::<Subst>();

        assert_eq!(
            substitution.len(),
            closure.owner_arguments().len(),
            "closure owner arguments must match enclosing definition's polymorphic variables"
        );

        let owner = MonoDefInstance::new(closure.owner_id(), substitution, engine).await;
        Self::new(owner, closure.local_closure_id())
    }
}
