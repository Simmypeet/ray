use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_solver::Solver;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    poly_var::get_enclosing_poly_var_maps,
    subst::{Subst, Substitutable},
    ty::{
        Ty,
        application::{ClosureID, ClosureView, NominalDropInstanceView, View as ApplicationView},
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
    /// they are not additional owner arguments. `solver` normalizes each
    /// argument and is shared so callers can reuse its state.
    #[must_use]
    pub async fn new(def_id: GlobalSymbolID, arguments: Subst, solver: &Solver) -> Self {
        // Empty keys already have no caller bindings or arguments to normalize.
        if arguments.is_empty() {
            return Self { def_id, substitution: arguments };
        }
        let engine = solver.engine();
        let maps = engine.get_enclosing_poly_var_maps(def_id).await;
        let mut substitution = Subst::new_empty();
        for variable in maps.all_poly_vars() {
            let value =
                engine.intern(Ty::PolyVar(variable)).apply_subst_or_clone(&arguments, engine);
            let value = solver.normalize(&value).await;

            // Lifetimes never affect code generation, so `f['a]` and `f['b]`
            // share one instance.
            let value = Ty::erase_lifetimes(&value, engine).await;
            assert!(
                value.recursive_iter().all(|ty| match ty {
                    Ty::Application(_) | Ty::EffectRow(_) | Ty::Lifetime(_) => true,
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
    /// The root function of a compiler-generated nominal Drop fragment.
    NominalDrop(MonoNominalDropInstance),
}

/// Identifies the compiler-generated `Drop.drop` body selected by one concrete
/// nominal Drop dictionary.
///
/// The key is the whole dictionary rather than only the nominal type: the
/// external dictionaries chosen by the caller, such as a lexical `Drop[t]`
/// override, decide how the fields are dropped.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct MonoNominalDropInstance {
    dictionary: Interned<Ty>,
}

impl MonoNominalDropInstance {
    /// Wraps a concrete `NominalDropInstance` dictionary. The dictionary is
    /// normalized, even when the caller already did so, and its lifetimes are
    /// erased. `solver` is shared so callers can reuse its state.
    pub async fn new(dictionary: &Interned<Ty>, solver: &Solver) -> Self {
        let dictionary = solver.normalize(dictionary).await;
        let dictionary = Ty::erase_lifetimes(&dictionary, solver.engine()).await;
        assert!(
            dictionary.recursive_iter().all(|ty| match ty {
                Ty::Application(_) | Ty::EffectRow(_) | Ty::Lifetime(_) => true,
                Ty::PolyVar(_) | Ty::Inference(_) | Ty::SelfInstance(_) => false,
            }),
            "nominal Drop dictionaries must be concrete: {dictionary:?}"
        );

        Self { dictionary }
    }

    /// The nominal type and the external dictionaries in plan requirement
    /// order.
    #[must_use]
    pub fn view(&self) -> NominalDropInstanceView<'_> {
        let Ty::Application(application) = &*self.dictionary else {
            panic!("a nominal Drop instance must be an application")
        };
        let ApplicationView::NominalDropInstance(view) = application.view() else {
            panic!("a nominal Drop instance must be a NominalDropInstance application")
        };
        view
    }
}

/// Identifies one independently lowered `MonoIR` fragment.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum MonoFragmentInstance {
    /// A source definition together with its nested lambdas and handlers.
    Definition(MonoDefInstance),
    /// A compiler-generated structural `Drop.drop` for a nominal type.
    NominalDrop(MonoNominalDropInstance),
}

impl From<MonoDefInstance> for MonoFragmentInstance {
    fn from(instance: MonoDefInstance) -> Self { Self::Definition(instance) }
}

impl From<MonoNominalDropInstance> for MonoFragmentInstance {
    fn from(instance: MonoNominalDropInstance) -> Self { Self::NominalDrop(instance) }
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

    pub async fn from_closure(solver: &Solver, closure: ClosureView<'_>) -> Self {
        let engine = solver.engine();
        let maps = engine.get_enclosing_poly_var_maps(closure.owner_id()).await;

        let variables = maps.all_poly_vars();
        let substitution =
            variables.zip(closure.owner_arguments().iter().cloned()).collect::<Subst>();

        assert_eq!(
            substitution.len(),
            closure.owner_arguments().len(),
            "closure owner arguments must match enclosing definition's polymorphic variables"
        );

        let owner = MonoDefInstance::new(closure.owner_id(), substitution, solver).await;
        Self::new(owner, closure.local_closure_id())
    }
}

/// A concrete instantiation of a source struct definition.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct MonoStructInstance {
    struct_id: GlobalSymbolID,
    substitution: Subst,
}

impl MonoStructInstance {
    #[must_use]
    pub const fn new(struct_id: GlobalSymbolID, substitution: Subst) -> Self {
        Self { struct_id, substitution }
    }

    #[must_use]
    pub const fn struct_id(&self) -> GlobalSymbolID { self.struct_id }

    #[must_use]
    pub const fn substitution(&self) -> &Subst { &self.substitution }
}
