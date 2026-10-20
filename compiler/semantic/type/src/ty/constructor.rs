use qbice::storage::intern::Interned;
use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;

use crate::{
    poly_var::{GlobalPolyVarID, get_poly_var_map},
    ty::{
        Mutability, Primitive, Ty, TyKind, application,
        application::{Application, Constant, View as ApplicationView},
        args::Args,
        effect_row,
        effect_row::EffectRow,
        lifetime::Lifetime,
    },
};

impl Ty {
    #[must_use]
    pub fn new_primitive(primitive: Primitive, engine: &TrackedEngine) -> Interned<Self> {
        engine.intern(Self::Application(Application::new(
            Constant::Primitive(primitive),
            engine.intern_unsized([]),
        )))
    }

    #[must_use]
    pub fn new_tuple(args: Interned<[Interned<Self>]>, engine: &TrackedEngine) -> Interned<Self> {
        engine.intern(Self::Application(Application::new(Constant::Tuple, args)))
    }

    /// Creates a nominal closure type, including its capture storage.
    /// Owner arguments precede the signature, with the owner's arguments first
    /// and each enclosing symbol's arguments following in declaration order.
    #[must_use]
    pub fn new_closure(
        closure: application::Closure,
        owner_arguments: impl IntoIterator<Item = Interned<Self>>,
        parameter_types: impl IntoIterator<Item = Interned<Self>>,
        return_type: Interned<Self>,
        effect_row: Interned<Self>,
        captured_tuple: Interned<Self>,
        engine: &TrackedEngine,
    ) -> Interned<Self> {
        let mut args = owner_arguments.into_iter().collect::<Vec<_>>();
        assert_eq!(args.len(), closure.owner_argument_count());
        args.extend(parameter_types.into_iter().chain([return_type, effect_row, captured_tuple]));
        engine.intern(Self::Application(Application::new(
            Constant::Closure(closure),
            engine.intern_unsized(args),
        )))
    }

    #[must_use]
    pub fn new_pointer(
        arg: Interned<Self>,
        mutability: Mutability,
        engine: &TrackedEngine,
    ) -> Interned<Self> {
        engine.intern(Self::Application(Application::new(
            Constant::Pointer(mutability),
            engine.intern_unsized([arg]),
        )))
    }

    /// Creates a checked reference `&'lifetime pointee`.
    #[must_use]
    pub fn new_reference(
        lifetime: Interned<Self>,
        pointee: Interned<Self>,
        mutability: Mutability,
        engine: &TrackedEngine,
    ) -> Interned<Self> {
        engine.intern(Self::Application(Application::new(
            Constant::Reference(mutability),
            engine.intern_unsized([lifetime, pointee]),
        )))
    }

    #[must_use]
    pub fn new_lifetime(lifetime: Lifetime, engine: &TrackedEngine) -> Interned<Self> {
        engine.intern(Self::Lifetime(lifetime))
    }

    #[must_use]
    pub async fn new_identity_struct(
        symbol_id: GlobalSymbolID,
        engine: &TrackedEngine,
    ) -> Interned<Self> {
        let params = engine.get_poly_var_map(symbol_id).await;

        Self::new_struct(
            symbol_id,
            Args::new(
                params.iter().map(|(id, _)| {
                    engine.intern(Self::PolyVar(GlobalPolyVarID::new(symbol_id, id)))
                }),
                engine,
            ),
            engine,
        )
    }

    #[must_use]
    pub fn new_struct(
        symbol_id: GlobalSymbolID,
        args: Args,
        engine: &TrackedEngine,
    ) -> Interned<Self> {
        engine.intern(Self::Application(Application::new(
            Constant::Struct(symbol_id),
            args.into_interned(),
        )))
    }

    /// Creates the built-in `Def` dictionary for a nominal closure type.
    #[must_use]
    pub fn new_def_instance(closure: Interned<Self>, engine: &TrackedEngine) -> Interned<Self> {
        assert!(closure.as_closure_view().is_some(), "Def instance requires a closure type");
        engine.intern(Self::Application(Application::new(
            Constant::DefInstance,
            engine.intern_unsized([closure]),
        )))
    }

    /// Creates the built-in no-op `Drop` dictionary for primitives, pointers,
    /// references, and `core.NoDrop[t]`.
    #[must_use]
    pub fn new_no_op_drop_instance(ty: Interned<Self>, engine: &TrackedEngine) -> Interned<Self> {
        assert!(
            matches!(
                &*ty,
                Self::Application(application)
                    if matches!(
                        application.view(),
                        ApplicationView::Primitive(_)
                            | ApplicationView::Pointer(_)
                            | ApplicationView::Reference(_)
                            | ApplicationView::Struct(_)
                    )
            ),
            "no-op Drop instance requires a compiler-provided no-op type"
        );
        engine.intern(Self::Application(Application::new(
            Constant::NoOpDropInstance,
            engine.intern_unsized([ty]),
        )))
    }

    /// Creates the built-in `Drop` dictionary for a tuple from the selected
    /// dictionary for each element.
    #[must_use]
    pub fn new_tuple_drop_instance(
        tuple: Interned<Self>,
        element_instances: impl IntoIterator<Item = Interned<Self>>,
        engine: &TrackedEngine,
    ) -> Interned<Self> {
        let Self::Application(application) = &*tuple else {
            panic!("tuple Drop instance requires a tuple type")
        };
        let ApplicationView::Tuple(tuple_view) = application.view() else {
            panic!("tuple Drop instance requires a tuple type")
        };

        // Keep the target tuple beside its selected element dictionaries so
        // lexical dictionaries survive substitution and monomorphization.
        let element_instances = element_instances.into_iter().collect::<Vec<_>>();
        assert_eq!(
            tuple_view.args().len(),
            element_instances.len(),
            "tuple Drop instance requires one dictionary per element"
        );
        let args = std::iter::once(tuple).chain(element_instances);
        engine.intern(Self::Application(Application::new(
            Constant::TupleDropInstance,
            engine.intern_unsized(args.collect::<Vec<_>>()),
        )))
    }

    /// Creates the built-in `Drop` dictionary for a closure from the selected
    /// dictionary for each capture.
    #[must_use]
    pub fn new_closure_drop_instance(
        closure: Interned<Self>,
        capture_instances: impl IntoIterator<Item = Interned<Self>>,
        engine: &TrackedEngine,
    ) -> Interned<Self> {
        let closure_view =
            closure.as_closure_view().expect("closure Drop instance requires a closure type");
        let captures = closure_view
            .captured_tuple()
            .as_tuple_view()
            .expect("closure Drop instance requires resolved captures")
            .args()
            .len();

        // Keep the closure beside its selected capture dictionaries so lexical
        // dictionaries survive substitution and monomorphization.
        let capture_instances = capture_instances.into_iter().collect::<Vec<_>>();
        assert_eq!(
            captures,
            capture_instances.len(),
            "closure Drop instance requires one dictionary per capture"
        );
        let args = std::iter::once(closure).chain(capture_instances);
        engine.intern(Self::Application(Application::new(
            Constant::ClosureDropInstance,
            engine.intern_unsized(args.collect::<Vec<_>>()),
        )))
    }

    /// Creates a generated nominal `Drop` dictionary. The selected external
    /// dictionaries follow the plan's requirement order, not field order.
    #[must_use]
    pub fn new_nominal_drop_instance(
        nominal: Interned<Self>,
        external_instances: impl IntoIterator<Item = Interned<Self>>,
        engine: &TrackedEngine,
    ) -> Interned<Self> {
        assert!(nominal.as_struct_view().is_some(), "nominal Drop instance requires a struct type");

        let args = std::iter::once(nominal).chain(external_instances);
        engine.intern(Self::Application(Application::new(
            Constant::NominalDropInstance,
            engine.intern_unsized(args.collect::<Vec<_>>()),
        )))
    }

    /// Creates the type of a suspended `def_id`. `poly_args` are the arguments
    /// for the polymorphic parameters of `def_id`, followed by those of each
    /// enclosing symbol, proceeding outwards.
    #[must_use]
    pub fn new_def_coroutine(
        def_id: GlobalSymbolID,
        poly_args: impl IntoIterator<Item = Interned<Self>>,
        engine: &TrackedEngine,
    ) -> Interned<Self> {
        engine.intern(Self::Application(Application::new(
            Constant::DefCoroutine(def_id),
            engine.intern_unsized(poly_args.into_iter().collect::<Vec<_>>()),
        )))
    }

    /// Creates the type of a suspended call of the trait method
    /// `trait_def_id` through `instance`. `args` are the arguments for the
    /// method's own polymorphic parameters, in declaration order.
    #[must_use]
    pub fn new_trait_def_coroutine(
        trait_def_id: GlobalSymbolID,
        instance: Interned<Self>,
        args: impl IntoIterator<Item = Interned<Self>>,
        engine: &TrackedEngine,
    ) -> Interned<Self> {
        let args = std::iter::once(instance).chain(args).collect::<Vec<_>>();
        engine.intern(Self::Application(Application::new(
            Constant::TraitDefCoroutine(trait_def_id),
            engine.intern_unsized(args),
        )))
    }

    /// Creates the built-in `Coroutine` dictionary for a coroutine type, or
    /// for a `&mut` to one.
    #[must_use]
    pub fn new_coroutine_instance(
        coroutine: Interned<Self>,
        engine: &TrackedEngine,
    ) -> Interned<Self> {
        assert!(
            coroutine.as_coroutine_behind_reference().is_some(),
            "Coroutine instance requires a coroutine type or a `&mut` to one"
        );
        engine.intern(Self::Application(Application::new(
            Constant::CoroutineInstance,
            engine.intern_unsized([coroutine]),
        )))
    }

    /// Creates the built-in `Def` dictionary of the `core.Continuation[r, c]`
    /// type `continuation`, from the `Coroutine` dictionary selected for `c`.
    ///
    /// The caller is responsible for `continuation` being an instantiation
    /// of `core.Continuation`; only its shape is checked here.
    #[must_use]
    pub fn new_continuation_def_instance(
        continuation: Interned<Self>,
        coroutine_instance: Interned<Self>,
        engine: &TrackedEngine,
    ) -> Interned<Self> {
        assert!(
            continuation.as_struct_view().is_some_and(|view| view.args().len() == 2),
            "continuation Def instance requires a `Continuation[r, c]` type"
        );
        engine.intern(Self::Application(Application::new(
            Constant::ContinuationDefInstance,
            engine.intern_unsized([continuation, coroutine_instance]),
        )))
    }

    /// Creates the built-in `Drop` dictionary for a coroutine type from the
    /// selected dictionary for each upvar, in parameter order.
    #[must_use]
    pub fn new_coroutine_drop_instance(
        coroutine: Interned<Self>,
        upvar_instances: impl IntoIterator<Item = Interned<Self>>,
        engine: &TrackedEngine,
    ) -> Interned<Self> {
        assert!(
            coroutine.as_coroutine_view().is_some(),
            "coroutine Drop instance requires a coroutine type"
        );

        // Keep the coroutine beside its selected upvar dictionaries so lexical
        // dictionaries survive substitution and monomorphization.
        let args = std::iter::once(coroutine).chain(upvar_instances);
        engine.intern(Self::Application(Application::new(
            Constant::CoroutineDropInstance,
            engine.intern_unsized(args.collect::<Vec<_>>()),
        )))
    }

    #[must_use]
    pub fn new_instance(
        symbol_id: GlobalSymbolID,
        args: Args,
        engine: &TrackedEngine,
    ) -> Interned<Self> {
        engine.intern(Self::Application(Application::new(
            Constant::Instance(symbol_id),
            args.into_interned(),
        )))
    }

    /// Creates a projection from an instance to a trait associated type.
    ///
    /// `symbol_id` must identify a `SymbolKind::TraitType`, `instance` must
    /// have kind `Instance`, and `args` must match the trait type's poly vars.
    #[must_use]
    pub fn new_instance_associated(
        symbol_id: GlobalSymbolID,
        instance: Interned<Self>,
        args: impl IntoIterator<Item = Interned<Self>>,
        engine: &TrackedEngine,
    ) -> Interned<Self> {
        let args = std::iter::once(instance).chain(args).collect::<Vec<_>>();
        engine.intern(Self::Application(Application::new(
            Constant::InstanceAssociated(symbol_id),
            engine.intern_unsized(args),
        )))
    }

    /// Creates an error of the given kind.
    #[must_use]
    pub fn new_error(kind: TyKind, engine: &TrackedEngine) -> Interned<Self> {
        engine.intern(Self::Application(Application::new(
            Constant::Error(kind),
            engine.intern_unsized([]),
        )))
    }

    #[must_use]
    pub fn new_star_error(engine: &TrackedEngine) -> Interned<Self> {
        Self::new_error(TyKind::Star, engine)
    }

    #[must_use]
    pub fn new_unit(engine: &TrackedEngine) -> Interned<Self> {
        engine
            .intern(Self::Application(Application::new(Constant::Tuple, engine.intern_unsized([]))))
    }

    #[must_use]
    pub fn new_poly_var(id: GlobalPolyVarID, engine: &TrackedEngine) -> Interned<Self> {
        engine.intern(Self::PolyVar(id))
    }

    #[must_use]
    pub fn new_effect_row(
        labels: impl IntoIterator<Item = Interned<effect_row::EffectLabel>>,
        tail: Option<Interned<Self>>,
        engine: &TrackedEngine,
    ) -> Interned<Self> {
        engine.intern(Self::EffectRow(EffectRow::new(labels, tail, engine)))
    }
}
