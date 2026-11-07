use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_ir::{get_ir, ir_closure::CaptureMapID, ir_expr::IRExprID, ir_function::IRFunctionID};
use rayc_qbice::TrackedEngine;
use rayc_solver::Solver;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    poly_var::get_enclosing_poly_var_maps,
    subst::Subst,
    ty::{
        Ty,
        application::{
            ClosureView, ConcreteCoroutineView, GeneratedAdtDropInstanceView,
            View as ApplicationView,
        },
    },
};

/// Identifies one function of the IR of a source definition, in one concrete
/// instantiation of that definition: the definition itself, the body of one of
/// its closures, the body of a `run ... with`, or one of its operation
/// handlers.
///
/// Every function of the IR is lowered on its own, so this is also the key of
/// the `MonoFunction` it is lowered into. An `extern def` has no IR, so it is
/// never one: see [`MonoFunctionInstance::ExternDef`].
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct MonoBodyInstance {
    def_id: GlobalSymbolID,
    substitution: Subst,
    ir_function_id: IRFunctionID,
}

impl MonoBodyInstance {
    /// The function `ir_function_id` of the IR of `def_id`, instantiated with
    /// `arguments`.
    ///
    /// Retains only the definition's semantic arguments, with normalized
    /// concrete values. Trait `SelfInstance` binders have already been replaced
    /// by `InstanceMember` correspondence before instance-method IR is built;
    /// they are not additional definition arguments. `solver` normalizes each
    /// argument and is shared so callers can reuse its state.
    #[must_use]
    pub async fn new(
        def_id: GlobalSymbolID,
        ir_function_id: IRFunctionID,
        arguments: Subst,
        solver: &Solver,
    ) -> Self {
        // Empty keys already have no caller bindings or arguments to normalize.
        if arguments.is_empty() {
            return Self { def_id, substitution: arguments, ir_function_id };
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

        Self { def_id, substitution, ir_function_id }
    }

    /// The definition function of `def_id`, which must have a body,
    /// instantiated with `arguments`.
    #[must_use]
    pub async fn new_definition(def_id: GlobalSymbolID, arguments: Subst, solver: &Solver) -> Self {
        let root_id = solver.engine().get_ir(def_id).await.root_id();
        Self::new(def_id, root_id, arguments, solver).await
    }

    /// The body of the concrete closure `closure`, which is a function of the
    /// IR of its owner.
    pub async fn from_closure(solver: &Solver, closure: ClosureView<'_>) -> Self {
        let engine = solver.engine();
        let source = engine.get_ir(closure.owner_id()).await;
        let ir_function_id = source.get_function_id_of_closure(closure.local_closure_id());
        let substitution = closure.create_subst(engine).await;

        Self::new(closure.owner_id(), ir_function_id, substitution, solver).await
    }

    /// Another function of the same IR, in the same instantiation, such as the
    /// body of a `run ... with` that this function creates.
    #[must_use]
    pub fn with_ir_function(&self, ir_function_id: IRFunctionID) -> Self {
        Self { def_id: self.def_id, substitution: self.substitution.clone(), ir_function_id }
    }

    /// The definition whose IR holds the function.
    #[must_use]
    pub const fn def_id(&self) -> GlobalSymbolID { self.def_id }

    /// The instantiation of the definition, which every function of its IR
    /// shares.
    #[must_use]
    pub const fn substitution(&self) -> &Subst { &self.substitution }

    /// The function in the IR of the definition.
    #[must_use]
    pub const fn ir_function_id(&self) -> IRFunctionID { self.ir_function_id }
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

/// Identifies one concrete coroutine: the suspended form of the function of a
/// definition or a closure.
///
/// The suspended function alone tells the frame and the signature of the
/// coroutine.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct MonoCoroutineInstance(MonoBodyInstance);

impl MonoCoroutineInstance {
    /// The coroutine that suspends `body`.
    #[must_use]
    pub const fn new(body: MonoBodyInstance) -> Self { Self(body) }

    /// The coroutine of the concrete coroutine type `coroutine`. The type is
    /// normalized, even when the caller already did so. `solver` is shared so
    /// callers can reuse its state.
    pub async fn from_type(coroutine: &Interned<Ty>, solver: &Solver) -> Self {
        let engine = solver.engine();
        let coroutine = solver.normalize(coroutine).await;
        debug_assert!(coroutine.is_concrete());

        let view = coroutine
            .as_concrete_coroutine_view()
            .unwrap_or_else(|| panic!("a coroutine must suspend a known function: {coroutine:?}"));
        let body = match view {
            ConcreteCoroutineView::Def(coroutine) => {
                let substitution = coroutine.create_subst(engine).await;
                MonoBodyInstance::new_definition(coroutine.def_id(), substitution, solver).await
            }
            ConcreteCoroutineView::Closure(coroutine) => {
                MonoBodyInstance::from_closure(solver, coroutine.closure_view()).await
            }
        };

        Self(body)
    }

    /// The function that the coroutine suspends.
    #[must_use]
    pub const fn body(&self) -> &MonoBodyInstance { &self.0 }
}

/// Identifies one `run ... with` expression of a concrete function, which
/// suspends the `coroutine_resume` of that function.
///
/// While the function is suspended there, it holds the frames of the handled
/// body and of the clause that runs, with the environment the clauses share,
/// in one aggregate; see [`RunWithLayout`].
///
/// [`RunWithLayout`]: crate::layout::run_with::RunWithLayout
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct MonoRunWithInstance {
    owner: MonoBodyInstance,
    expression: IRExprID,
}

impl MonoRunWithInstance {
    /// The `run ... with` expression `expression` of the function `owner`.
    #[must_use]
    pub const fn new(owner: MonoBodyInstance, expression: IRExprID) -> Self {
        Self { owner, expression }
    }

    /// The function that evaluates the `run ... with`, whose IR also holds
    /// its body and its clauses.
    #[must_use]
    pub const fn owner(&self) -> &MonoBodyInstance { &self.owner }

    /// The `run ... with` expression in the function of the owner.
    #[must_use]
    pub const fn expression(&self) -> IRExprID { self.expression }
}

/// Identifies one independently lowered `MonoFunction`, which is also how a
/// function refers to another it calls or takes the address of.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum MonoFunctionInstance {
    /// A function of the IR of a source definition, in callback style.
    Body(MonoBodyInstance),
    /// A function that the C environment provides, which is only declared. It
    /// cannot be polymorphic, so the symbol alone names it.
    ExternDef(GlobalSymbolID),
    /// A compiler-generated structural `Drop.drop` for a nominal type.
    NominalDrop(MonoGeneratedAdtDropInstance),
    /// The `coroutine_resume` function of a coroutine.
    CoroutineResume(MonoCoroutineInstance),
}

impl From<MonoBodyInstance> for MonoFunctionInstance {
    fn from(instance: MonoBodyInstance) -> Self { Self::Body(instance) }
}

impl From<MonoGeneratedAdtDropInstance> for MonoFunctionInstance {
    fn from(instance: MonoGeneratedAdtDropInstance) -> Self { Self::NominalDrop(instance) }
}

impl From<MonoCoroutineInstance> for MonoFunctionInstance {
    fn from(instance: MonoCoroutineInstance) -> Self { Self::CoroutineResume(instance) }
}

/// The capture environment of one capture map in one concrete definition
/// instantiation: the inline environment of a closure, which is also the value
/// type of the closure, or the environment that the body of a `run ... with`
/// or the clauses of one handler record share.
///
/// It is nominal, so lowering an environment type never lowers its captures,
/// which an [`EnvironmentLayout`] tells.
///
/// [`EnvironmentLayout`]: crate::layout::EnvironmentLayout
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct MonoEnvironmentInstance {
    def_id: GlobalSymbolID,
    substitution: Subst,
    capture_map: CaptureMapID,
}

impl MonoEnvironmentInstance {
    /// The environment of `capture_map`, a capture map of the IR that `body`
    /// is a function of, in the instantiation of `body`.
    #[must_use]
    pub fn new(body: &MonoBodyInstance, capture_map: CaptureMapID) -> Self {
        Self { def_id: body.def_id(), substitution: body.substitution().clone(), capture_map }
    }

    /// The environment that `body` receives, if it has captures. A definition
    /// function has none.
    pub async fn of_body(body: &MonoBodyInstance, engine: &TrackedEngine) -> Option<Self> {
        let source = engine.get_ir(body.def_id()).await;
        source
            .try_get_capture_map_id(body.ir_function_id())
            .map(|capture_map| Self::new(body, capture_map))
    }

    /// The definition whose IR holds the captures of the environment.
    #[must_use]
    pub const fn def_id(&self) -> GlobalSymbolID { self.def_id }

    /// The instantiation of the definition that the captures are lowered in.
    #[must_use]
    pub const fn substitution(&self) -> &Subst { &self.substitution }

    /// The capture map, in the IR of the definition, that the environment
    /// holds.
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
