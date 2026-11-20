use qbice::storage::intern::Interned;
use rayc_qbice::TrackedEngine;
use rayc_symbol::{
    GlobalSymbolID,
    symbol_kind::{SymbolKind, get_symbol_kind},
};

use crate::{
    poly_var::{GlobalPolyVarID, get_enclosing_poly_var_maps, get_poly_var_map},
    subst::Subst,
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

    #[must_use]
    pub fn new_enum(
        symbol_id: GlobalSymbolID,
        args: Args,
        engine: &TrackedEngine,
    ) -> Interned<Self> {
        engine.intern(Self::Application(Application::new(
            Constant::Enum(symbol_id),
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

    /// Creates a generated `Drop` dictionary for a struct or an enum, with its
    /// external dictionaries in the plan's requirement order.
    #[must_use]
    pub fn new_generated_adt_drop_instance(
        nominal: Interned<Self>,
        external_instances: impl IntoIterator<Item = Interned<Self>>,
        engine: &TrackedEngine,
    ) -> Interned<Self> {
        assert!(
            nominal.as_struct_view().is_some() || nominal.as_enum_view().is_some(),
            "nominal Drop instance requires a struct or an enum type"
        );

        let args = std::iter::once(nominal).chain(external_instances);
        engine.intern(Self::Application(Application::new(
            Constant::GeneratedAdtDropInstance,
            engine.intern_unsized(args.collect::<Vec<_>>()),
        )))
    }

    /// Creates the type of a suspended `def_id`, applied to its and its
    /// enclosing symbols' arguments.
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

    /// Creates the type of a suspended call of the trait method `trait_def_id`
    /// through `instance`, reduced once the instance is known.
    pub async fn new_trait_def_coroutine(
        trait_def_id: GlobalSymbolID,
        instance: Interned<Self>,
        args: impl IntoIterator<Item = Interned<Self>>,
        engine: &TrackedEngine,
    ) -> Interned<Self> {
        let args = std::iter::once(instance).chain(args).collect::<Vec<_>>();
        Self::new_application(
            Application::new(
                Constant::TraitDefCoroutine(trait_def_id),
                engine.intern_unsized(args),
            ),
            engine,
        )
        .await
    }

    /// Creates the type of a suspended call of `closure`; see
    /// [`Constant::ClosureCoroutine`].
    #[must_use]
    pub fn new_closure_coroutine(
        closure: Interned<Self>,
        engine: &TrackedEngine,
    ) -> Interned<Self> {
        assert!(closure.as_closure_view().is_some(), "closure coroutine requires a closure type");
        engine.intern(Self::Application(Application::new(
            Constant::ClosureCoroutine,
            engine.intern_unsized([closure]),
        )))
    }

    /// Returns the coroutine type suspending a direct call of `function_id`, or
    /// `None` when the callee has no body to suspend.
    pub async fn new_direct_call_coroutine(
        function_id: GlobalSymbolID,
        call_subst: &Subst,
        engine: &TrackedEngine,
    ) -> Option<Interned<Self>> {
        if !matches!(
            engine.get_symbol_kind(function_id).await,
            SymbolKind::Def | SymbolKind::InstanceDef
        ) {
            return None;
        }

        // The substitution of a suspendable callee covers all of them: a
        // `def` is enclosed by no polymorphic symbol, and the substitution of
        // an instance's `def` includes the arguments of the instance.
        let poly_vars = engine.get_enclosing_poly_var_maps(function_id).await;
        let poly_args = poly_vars
            .create_args_from_subst(call_subst)
            .expect("the call should instantiate every parameter of the coroutine's function");

        Some(Self::new_def_coroutine(function_id, poly_args, engine))
    }

    /// Returns the coroutine type suspending a call of `trait_def_id` through
    /// `instance`, whichever implementation it turns out to have.
    pub async fn new_instance_call_coroutine(
        trait_def_id: GlobalSymbolID,
        instance: Interned<Self>,
        call_subst: &Subst,
        engine: &TrackedEngine,
    ) -> Interned<Self> {
        // The trait def's own parameters are the arguments; the instance
        // tells those of its trait.
        let poly_vars = engine.get_poly_var_map(trait_def_id).await;
        let args = poly_vars
            .args_from_subst(trait_def_id, call_subst)
            .expect("the call should instantiate every parameter of its trait def");

        Self::new_trait_def_coroutine(trait_def_id, instance, args, engine).await
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

    /// Creates a projection from an instance to a trait associated type,
    /// reduced once the instance tells its definition.
    pub async fn new_instance_associated(
        symbol_id: GlobalSymbolID,
        instance: Interned<Self>,
        args: impl IntoIterator<Item = Interned<Self>>,
        engine: &TrackedEngine,
    ) -> Interned<Self> {
        let args = std::iter::once(instance).chain(args).collect::<Vec<_>>();
        Self::new_application(
            Application::new(Constant::InstanceAssociated(symbol_id), engine.intern_unsized(args)),
            engine,
        )
        .await
    }

    /// Interns `application`, reducing it if it is a projection whose instance
    /// is known; its arguments must already be reduced.
    pub(crate) async fn new_application(
        application: Application,
        engine: &TrackedEngine,
    ) -> Interned<Self> {
        // Expanding a projection substitutes into its definition, which
        // constructs projections again, so the future is boxed. A definition
        // may name the projection itself, which is no step.
        if application.is_projection()
            && let Some(reduced) = Box::pin(application.reduce_projection(engine)).await
            && !matches!(&*reduced, Self::Application(reduced) if *reduced == application)
        {
            return reduced;
        }

        engine.intern(Self::Application(application))
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

    /// Creates the effect row `{labels | tail}`, flattened so that its tail is
    /// never another effect row and a row of only a tail is that tail.
    #[must_use]
    pub fn new_effect_row(
        labels: impl IntoIterator<Item = Interned<effect_row::EffectLabel>>,
        tail: Option<Interned<Self>>,
        engine: &TrackedEngine,
    ) -> Interned<Self> {
        let mut labels = labels.into_iter().collect::<Vec<_>>();

        // A row tail contributes its labels and its own tail, which is never a
        // row since that row was flattened when it was constructed.
        let tail = match tail.as_deref() {
            Some(Self::EffectRow(tail_row)) => {
                labels.extend(tail_row.labels().cloned());
                tail_row.tail().cloned()
            }
            Some(_) | None => tail,
        };

        match (labels.is_empty(), tail) {
            (true, Some(tail)) => tail,
            (_, tail) => engine.intern(Self::EffectRow(EffectRow::new(labels, tail, engine))),
        }
    }
}
