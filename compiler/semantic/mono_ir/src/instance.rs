use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_ir::{get_ir, ir_closure::CaptureMapID};
use rayc_qbice::TrackedEngine;
use rayc_solver::Solver;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    poly_var::get_enclosing_poly_var_maps,
    subst::Subst,
    ty::{
        Ty,
        application::{
            ClosureID, ClosureView, ConcreteCoroutineView, GeneratedAdtDropInstanceView,
            View as ApplicationView,
        },
    },
};

use crate::function::{MonoFunctionID, MonoFunctionKind};

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
        let mut substitution = maps
            .restrict_subst(&arguments)
            .expect("definition arguments must instantiate every polymorphic variable");
        for value in substitution.codomain_mut() {
            let normalized = solver.normalize(value).await;

            // Lifetimes never affect code generation, so `f['a]` and `f['b]`
            // share one instance.
            *value = Ty::erase_lifetimes(&normalized, engine).await;
            debug_assert!(value.is_concrete());
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
    pub async fn new(effect_id: GlobalSymbolID, mut substitution: Subst, solver: &Solver) -> Self {
        for val in substitution.codomain_mut() {
            *val = solver.normalize(val).await;
            *val = Ty::erase_lifetimes(val, solver.engine()).await;

            debug_assert!(val.is_concrete());
        }

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
    NominalDrop(MonoGeneratedAdtDropInstance),
    /// The `coroutine_resume` function of a coroutine fragment.
    CoroutineResume(MonoCoroutineInstance),
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
pub struct MonoGeneratedAdtDropInstance {
    dictionary: Interned<Ty>,
}

impl MonoGeneratedAdtDropInstance {
    /// Wraps a concrete `GeneratedAdtDropInstance` dictionary. The dictionary
    /// is normalized, even when the caller already did so, and its
    /// lifetimes are erased. `solver` is shared so callers can reuse its
    /// state.
    pub async fn new(dictionary: &Interned<Ty>, solver: &Solver) -> Self {
        let dictionary = solver.normalize(dictionary).await;
        let dictionary = Ty::erase_lifetimes(&dictionary, solver.engine()).await;
        debug_assert!(dictionary.is_concrete());

        Self { dictionary }
    }

    /// The nominal type and the external dictionaries in plan requirement
    /// order.
    #[must_use]
    pub fn view(&self) -> GeneratedAdtDropInstanceView<'_> {
        let Ty::Application(application) = &*self.dictionary else {
            panic!("a nominal Drop instance must be an application")
        };
        let ApplicationView::GeneratedAdtDropInstance(view) = application.view() else {
            panic!("a nominal Drop instance must be a GeneratedAdtDropInstance application")
        };
        view
    }
}

/// Identifies one concrete coroutine: the suspended form of a definition or a
/// closure.
///
/// The key is the coroutine type itself. It alone tells the suspended function
/// and its signature, which a closure's owner and local ID would not.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct MonoCoroutineInstance {
    coroutine: Interned<Ty>,
}

impl MonoCoroutineInstance {
    /// Wraps a concrete coroutine type. The type is normalized, even when the
    /// caller already did so, and its lifetimes are erased. `solver` is shared
    /// so callers can reuse its state.
    pub async fn new(coroutine: &Interned<Ty>, solver: &Solver) -> Self {
        let coroutine = solver.normalize(coroutine).await;
        let coroutine = Ty::erase_lifetimes(&coroutine, solver.engine()).await;

        debug_assert!(coroutine.is_concrete());
        debug_assert!(
            coroutine.as_concrete_coroutine_view().is_some(),
            "a coroutine instance must suspend a known function: {coroutine:?}"
        );

        Self { coroutine }
    }

    /// The coroutine type, whose suspended function is known once it is
    /// concrete.
    #[must_use]
    pub fn view(&self) -> ConcreteCoroutineView<'_> {
        self.coroutine
            .as_concrete_coroutine_view()
            .expect("a coroutine instance must suspend a known function")
    }
}

/// Identifies one independently lowered `MonoIR` fragment.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum MonoFragmentInstance {
    /// A source definition together with the `run ... with` bodies and
    /// operation handlers it creates. A closure it creates is a fragment of its
    /// own.
    Definition(MonoDefInstance),
    /// A source closure together with the `run ... with` bodies and operation
    /// handlers it creates.
    Closure(MonoClosureInstance),
    /// A compiler-generated structural `Drop.drop` for a nominal type.
    NominalDrop(MonoGeneratedAdtDropInstance),
    /// The `coroutine_resume` function of a coroutine.
    CoroutineResume(MonoCoroutineInstance),
}

impl MonoFragmentInstance {
    /// The kind of the function at the root of this fragment.
    #[must_use]
    pub const fn root_kind(&self) -> MonoFunctionKind {
        match self {
            Self::Definition(_) | Self::NominalDrop(_) => MonoFunctionKind::Def,
            Self::Closure(_) => MonoFunctionKind::Closure,
            Self::CoroutineResume(_) => MonoFunctionKind::CoroutineResume,
        }
    }
}

impl From<MonoDefInstance> for MonoFragmentInstance {
    fn from(instance: MonoDefInstance) -> Self { Self::Definition(instance) }
}

impl From<MonoClosureInstance> for MonoFragmentInstance {
    fn from(instance: MonoClosureInstance) -> Self { Self::Closure(instance) }
}

impl From<MonoGeneratedAdtDropInstance> for MonoFragmentInstance {
    fn from(instance: MonoGeneratedAdtDropInstance) -> Self { Self::NominalDrop(instance) }
}

impl From<MonoCoroutineInstance> for MonoFragmentInstance {
    fn from(instance: MonoCoroutineInstance) -> Self { Self::CoroutineResume(instance) }
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

    #[must_use]
    pub const fn closure_id(&self) -> ClosureID { self.closure_id }

    pub async fn from_closure(solver: &Solver, closure: ClosureView<'_>) -> Self {
        let substitution = closure.create_subst(solver.engine()).await;
        let owner = MonoDefInstance::new(closure.owner_id(), substitution, solver).await;
        Self::new(owner, closure.local_closure_id())
    }
}

/// The capture environment of one capture map in one concrete owner
/// specialization: the inline environment of a closure, which is also the
/// value type of the closure, or the environment that the body of a
/// `run ... with` or the clauses of one handler record share.
///
/// It is nominal, so lowering an environment type never lowers its captures,
/// which an [`EnvironmentLayout`] tells.
///
/// [`EnvironmentLayout`]: crate::layout::EnvironmentLayout
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct MonoEnvironmentInstance {
    owner: MonoDefInstance,
    capture_map: CaptureMapID,
}

impl MonoEnvironmentInstance {
    /// The environment of `capture_map`, a capture map of the IR of `owner`.
    #[must_use]
    pub const fn new(owner: MonoDefInstance, capture_map: CaptureMapID) -> Self {
        Self { owner, capture_map }
    }

    /// The inline environment of `closure`, whose capture map is in the IR of
    /// its owner.
    pub async fn of_closure(closure: &MonoClosureInstance, engine: &TrackedEngine) -> Self {
        let source = engine.get_ir(closure.owner().def_id()).await;
        let function_id = source.get_function_id_of_closure(closure.closure_id());
        Self::new(closure.owner().clone(), source.get_capture_map_id(function_id))
    }

    /// The definition whose IR holds the captures of the environment.
    #[must_use]
    pub const fn owner(&self) -> &MonoDefInstance { &self.owner }

    /// The capture map, in the IR of the owner, that the environment holds.
    #[must_use]
    pub const fn capture_map(&self) -> CaptureMapID { self.capture_map }
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
    pub fn new(struct_id: GlobalSymbolID, substitution: Subst) -> Self {
        for subst in substitution.codomain() {
            debug_assert!(subst.is_concrete());
        }

        Self { struct_id, substitution }
    }

    #[must_use]
    pub const fn struct_id(&self) -> GlobalSymbolID { self.struct_id }

    #[must_use]
    pub const fn substitution(&self) -> &Subst { &self.substitution }
}
