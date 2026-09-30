use std::{
    collections::{VecDeque, hash_map::Entry},
    fmt::{self, Display, Write},
};

use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_hash::FxHashMap;
use rayc_qbice::TrackedEngine;
use rayc_symbol::{GlobalSymbolID, name::get_name};

use crate::{
    constraint::outlives::{OutlivesConstraint, OutlivesConstraints},
    poly_var::{GlobalPolyVarID, Key as PolyVarKey, PolyVarMap, get_poly_var_map},
    reduce::Reduce,
    rewrite::{RewriteAsync, TyRewriterAsync},
    subst::{Subst, Substitutable},
    ty::{
        application::{Application, Constant, InstanceView, StructView, View as ApplicationView},
        args::Args,
        effect_row::EffectRow,
        inference::{GenInfer, Inference},
        lifetime::{Lifetime, RegionID},
    },
    variance::Variance,
};

pub mod application;
pub mod args;
pub mod effect_row;
pub mod inference;
pub mod lifetime;
pub mod self_instance;

use self_instance::SelfInstance;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum Primitive {
    Int32,
    Float32,
    Bool,
    CInt,
    CStr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum Mutability {
    Immutable,
    Mutable,
}

impl Mutability {
    #[must_use]
    pub const fn constness(&self) -> bool {
        match self {
            Self::Immutable => true,
            Self::Mutable => false,
        }
    }

    /// Returns the variance of the pointee of a reference with this
    /// mutability: a shared pointee is covariant, and a mutable one is
    /// invariant. Raw pointers are bivariant in their pointee instead.
    #[must_use]
    pub const fn pointee_variance(&self) -> Variance {
        match self {
            Self::Immutable => Variance::Covariant,
            Self::Mutable => Variance::Invariant,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum TyKind {
    Star,
    EffectRow,
    Instance,
    Lifetime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum InferenceConstraint {
    Any,
    Numeric,
    EqualityComparable,
}

impl InferenceConstraint {
    #[must_use]
    #[allow(clippy::match_same_arms)]
    pub const fn meet(&self, other: &Self) -> Option<Self> {
        match (self, other) {
            (Self::Any, Self::Any) => Some(Self::Any),
            (Self::Any, Self::Numeric) | (Self::Numeric, Self::Any) => Some(Self::Numeric),
            (Self::Any, Self::EqualityComparable) | (Self::EqualityComparable, Self::Any) => {
                Some(Self::EqualityComparable)
            }
            (Self::Numeric, Self::Numeric) => Some(Self::Numeric),
            (Self::Numeric, Self::EqualityComparable)
            | (Self::EqualityComparable, Self::Numeric) => Some(Self::Numeric),
            (Self::EqualityComparable, Self::EqualityComparable) => Some(Self::EqualityComparable),
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum Ty {
    Application(Application),
    Inference(Inference),
    PolyVar(GlobalPolyVarID),
    /// The enclosing trait’s rigid self dictionary; see [`SelfInstance`].
    SelfInstance(SelfInstance),
    EffectRow(EffectRow),
    /// A lifetime that is not a lifetime parameter; see [`Lifetime`].
    Lifetime(Lifetime),
}

impl Ty {
    /// Returns whether this is a built-in `Drop` dictionary that does nothing.
    ///
    /// Tuple and closure dictionaries are no-ops when all their element or
    /// capture dictionaries are recursively no-ops, including empty ones.
    #[must_use]
    pub fn is_no_op_drop_instance(&self) -> bool {
        match self {
            Self::Application(application) => match application.view() {
                ApplicationView::NoOpDropInstance(_) => true,
                ApplicationView::TupleDropInstance(instance) => instance
                    .element_instances()
                    .iter()
                    .all(|instance| instance.is_no_op_drop_instance()),
                ApplicationView::ClosureDropInstance(instance) => instance
                    .capture_instances()
                    .iter()
                    .all(|instance| instance.is_no_op_drop_instance()),
                ApplicationView::Primitive(_)
                | ApplicationView::Tuple(_)
                | ApplicationView::Pointer(_)
                | ApplicationView::Reference(_)
                | ApplicationView::Struct(_)
                | ApplicationView::Instance(_)
                | ApplicationView::InstanceAssociated(_)
                | ApplicationView::Closure(_)
                | ApplicationView::DefInstance(_)
                | ApplicationView::NominalDropInstance(_)
                | ApplicationView::Error => false,
            },
            Self::Inference(_)
            | Self::PolyVar(_)
            | Self::SelfInstance(_)
            | Self::EffectRow(_)
            | Self::Lifetime(_) => false,
        }
    }

    /// Returns whether both types are applications of the same outer type
    /// constructor with the same arity.
    #[must_use]
    pub fn has_same_type_constructor(&self, other: &Self) -> bool {
        if let (Self::Application(left), Self::Application(right)) = (self, other) {
            left.has_same_constant(right)
        } else {
            false
        }
    }

    pub async fn kind_of(&self, engine: &TrackedEngine) -> TyKind {
        match self {
            Self::Application(application) => application.kind_of(engine).await,
            Self::Inference(inference) => inference.kind(),
            Self::PolyVar(poly_var) => {
                let poly_var_map = engine.get_poly_var_map(poly_var.parent_id()).await;
                poly_var_map.kind_of(poly_var.id())
            }
            Self::EffectRow(_) => TyKind::EffectRow,
            Self::SelfInstance(_) => TyKind::Instance,
            Self::Lifetime(_) => TyKind::Lifetime,
        }
    }

    /// Iterates over an interned type and all its recursively nested type
    /// arguments in breadth-first order.
    pub fn interned_recursive_iter(
        root: &Interned<Self>,
    ) -> impl Iterator<Item = &'_ Interned<Self>> {
        let mut pending = VecDeque::from([root]);

        std::iter::from_fn(move || {
            let ty = pending.pop_front()?;
            match &**ty {
                Self::Application(application) => pending.extend(application.interned_iter()),
                Self::EffectRow(row) => pending.extend(row.interned_iter()),
                Self::Inference(_)
                | Self::PolyVar(_)
                | Self::SelfInstance(_)
                | Self::Lifetime(_) => {}
            }
            Some(ty)
        })
    }

    pub fn recursive_iter(&self) -> impl Iterator<Item = &Self> {
        let mut pending = VecDeque::from([self]);

        std::iter::from_fn(move || {
            let ty = pending.pop_front()?;
            match ty {
                Self::Application(application) => {
                    pending.extend(application.iter());
                }
                Self::EffectRow(row) => pending.extend(row.iter()),
                Self::Inference(_)
                | Self::PolyVar(_)
                | Self::SelfInstance(_)
                | Self::Lifetime(_) => {}
            }
            Some(ty)
        })
    }

    /// Includes the root and all descendants; rigid polyvars are not inference.
    #[must_use]
    pub fn contains_inference(&self) -> bool {
        self.recursive_iter().any(|ty| match ty {
            Self::Inference(_) => true,
            Self::Application(_)
            | Self::EffectRow(_)
            | Self::PolyVar(_)
            | Self::SelfInstance(_)
            | Self::Lifetime(_) => false,
        })
    }

    /// Returns whether this type or a descendant is an inference variable
    /// that is not a lifetime.
    ///
    /// Lifetimes never decide which instance is selected, so a lifetime
    /// inference variable does not keep a requirement from being resolved.
    #[must_use]
    pub fn contains_non_lifetime_inference(&self) -> bool {
        self.recursive_iter().any(|ty| match ty {
            Self::Inference(inference) => inference.kind() != TyKind::Lifetime,
            Self::Application(_)
            | Self::EffectRow(_)
            | Self::PolyVar(_)
            | Self::SelfInstance(_)
            | Self::Lifetime(_) => false,
        })
    }

    /// Includes errors of every kind at the root or in any descendant.
    #[must_use]
    pub fn contains_error(&self) -> bool {
        self.recursive_iter().any(|ty| match ty {
            Self::Application(application) => match application.view() {
                ApplicationView::Error => true,
                ApplicationView::Primitive(_)
                | ApplicationView::Tuple(_)
                | ApplicationView::Pointer(_)
                | ApplicationView::Reference(_)
                | ApplicationView::Struct(_)
                | ApplicationView::InstanceAssociated(_)
                | ApplicationView::Closure(_)
                | ApplicationView::DefInstance(_)
                | ApplicationView::NoOpDropInstance(_)
                | ApplicationView::TupleDropInstance(_)
                | ApplicationView::ClosureDropInstance(_)
                | ApplicationView::NominalDropInstance(_)
                | ApplicationView::Instance(_) => false,
            },
            Self::Inference(_)
            | Self::EffectRow(_)
            | Self::PolyVar(_)
            | Self::SelfInstance(_)
            | Self::Lifetime(_) => false,
        })
    }

    #[must_use]
    pub fn has_inference_variable(&self, ty: &Inference) -> bool {
        match self {
            Self::Application(application) => application.has_inference_variable(ty),
            Self::Inference(ty_inference) => ty_inference == ty,
            Self::EffectRow(row) => row.has_inference_variable(ty),
            Self::PolyVar(_) | Self::SelfInstance(_) | Self::Lifetime(_) => false,
        }
    }

    /// Replaces every lifetime, lifetime parameters included, with
    /// [`Lifetime::Erased`].
    ///
    /// Lifetimes never affect code generation, so monomorphization erases
    /// them before it interns an instantiated type: `f['a]` and `f['b]` must
    /// share one instance, and `Ref['static, t]` and `Ref['a, t]` must lower
    /// to one type.
    pub async fn erase_lifetimes(ty: &Interned<Self>, engine: &TrackedEngine) -> Interned<Self> {
        ty.rewrite_async_or_clone(&mut LifetimeEraser { engine }, engine).await
    }

    /// Returns whether this type is of kind [`TyKind::Lifetime`]: a
    /// lifetime, a lifetime parameter, or an error of that kind.
    ///
    /// Lifetime inference variables only come from generalization during type
    /// inference. The borrow checker's region variables are
    /// [`Lifetime::Region`]s instead.
    pub async fn is_lifetime(&self, engine: &TrackedEngine) -> bool {
        self.kind_of(engine).await == TyKind::Lifetime
    }

    /// Returns whether this type is a universal lifetime: `'static`, a
    /// lifetime parameter or an external lifetime, a region which the
    /// function mentioning it does not choose, but is given; see
    /// [`Lifetime::is_universal`].
    pub async fn is_universal_lifetime(&self, engine: &TrackedEngine) -> bool {
        match self {
            Self::Lifetime(lifetime) => lifetime.is_universal(),
            Self::PolyVar(_) => self.is_lifetime(engine).await,
            Self::Application(_)
            | Self::Inference(_)
            | Self::SelfInstance(_)
            | Self::EffectRow(_) => false,
        }
    }

    /// Returns whether `left` and `right` are equal when every lifetime is
    /// considered equal to every other lifetime.
    ///
    /// Effect rows are matched as scoped labels, as by
    /// [`EffectRow::match_labels`], so labels of different effects commute.
    ///
    /// Returns `None` if they are not equal. Otherwise, returns the outlives
    /// constraints of relating each pair of corresponding lifetimes
    /// invariantly.
    pub async fn equal_modulo_lifetimes(
        left: &Interned<Self>,
        right: &Interned<Self>,
        engine: &TrackedEngine,
    ) -> Option<OutlivesConstraints> {
        let mut lifetimes = Vec::new();
        let mut pending = vec![(left.clone(), right.clone())];

        while let Some((left, right)) = pending.pop() {
            if left == right {
                continue;
            }

            match (&*left, &*right) {
                // The same constructor applied to arguments that are equal
                // modulo lifetimes.
                (Self::Application(left_application), Self::Application(right_application))
                    if left_application.has_same_constant(right_application) =>
                {
                    let arguments = left_application.structural_match(right_application)?;
                    pending.extend(arguments.map(|(left, right)| (left.clone(), right.clone())));
                }

                // Rows whose labels all match, argument by argument, and whose
                // tails are equal modulo lifetimes.
                (Self::EffectRow(left_row), Self::EffectRow(right_row)) => {
                    let labels = left_row.match_labels(right_row);
                    if !labels.is_exact() {
                        return None;
                    }
                    for (left_label, right_label) in labels.matched() {
                        let arguments = left_label.structural_match(right_label)?;
                        pending
                            .extend(arguments.map(|(left, right)| (left.clone(), right.clone())));
                    }
                    match (left_row.tail(), right_row.tail()) {
                        (None, None) => {}
                        (Some(left), Some(right)) => pending.push((left.clone(), right.clone())),
                        (None, Some(_)) | (Some(_), None) => return None,
                    }
                }

                // Any two lifetimes are equal; nothing else differs and is
                // still equal.
                _ => {
                    if !left.is_lifetime(engine).await || !right.is_lifetime(engine).await {
                        return None;
                    }
                    lifetimes.push((left, right));
                }
            }
        }

        Some(
            lifetimes
                .iter()
                .flat_map(|(left, right)| {
                    OutlivesConstraint::from_relation(left, right, Variance::Invariant)
                })
                .collect(),
        )
    }

    #[must_use]
    pub fn has_poly_variable(&self, poly_var: &GlobalPolyVarID) -> bool {
        self.recursive_iter().any(|ty| match ty {
            Self::PolyVar(ty_poly_var) => ty_poly_var == poly_var,
            Self::Application(_)
            | Self::Inference(_)
            | Self::EffectRow(_)
            | Self::SelfInstance(_)
            | Self::Lifetime(_) => false,
        })
    }
}

/// The [`TyRewriterAsync`] behind [`Ty::erase_lifetimes`]. It is async
/// because telling a lifetime parameter from other polymorphic variables needs
/// the kind recorded in its poly var map.
struct LifetimeEraser<'e> {
    engine: &'e TrackedEngine,
}

impl TyRewriterAsync for LifetimeEraser<'_> {
    async fn rewrite(&mut self, ty: &Interned<Ty>) -> Option<Interned<Ty>> {
        let erased = || Ty::new_lifetime(Lifetime::Erased, self.engine);
        match &**ty {
            Ty::Lifetime(Lifetime::Static | Lifetime::Region(_) | Lifetime::External(_)) => {
                Some(erased())
            }
            Ty::PolyVar(poly_var) => {
                let poly_var_map = self.engine.get_poly_var_map(poly_var.parent_id()).await;
                (poly_var_map.kind_of(poly_var.id()) == TyKind::Lifetime).then(erased)
            }
            Ty::Lifetime(Lifetime::Erased)
            | Ty::Application(_)
            | Ty::Inference(_)
            | Ty::SelfInstance(_)
            | Ty::EffectRow(_) => None,
        }
    }
}

impl Substitutable for Interned<Ty> {
    fn apply_subst(&self, subst: &Subst, engine: &TrackedEngine) -> Option<Self>
    where
        Self: Sized,
    {
        match &**self {
            Ty::Application(application) => application
                .apply_subst(subst, engine)
                .map(|application| engine.intern(Ty::Application(application))),

            Ty::Inference(ty_inference) => subst.get(ty_inference).cloned(),
            Ty::PolyVar(poly) => subst.get(poly).cloned(),
            Ty::SelfInstance(instance) => subst.get(instance).cloned(),
            Ty::EffectRow(row) => {
                row.apply_subst(subst, engine).map(|new_row| engine.intern(Ty::EffectRow(new_row)))
            }
            Ty::Lifetime(Lifetime::External(external)) => subst.get(external).cloned(),
            Ty::Lifetime(Lifetime::Static | Lifetime::Erased | Lifetime::Region(_)) => None,
        }
    }
}

impl Reduce for Interned<Ty> {
    async fn reduce(
        &self,
        engine: &TrackedEngine,
        givens: &[crate::where_clause::PredicateKind],
    ) -> Option<(Self, OutlivesConstraints)> {
        // Prefer structural reduction, then the first matching given equality.
        if let Some(reduced) = reduce_type(self, engine, givens).await {
            return Some(reduced);
        }
        for equality in givens.iter().filter_map(crate::where_clause::PredicateKind::as_equality) {
            // Lifetimes never decide whether a given applies, just as they
            // never decide which instance is selected. Projection arguments
            // are invariant, so the lifetimes of a matching given are related
            // by equality.
            if equality.right() == self {
                continue;
            }
            if let Some(outlives) = Ty::equal_modulo_lifetimes(equality.left(), self, engine).await
            {
                return Some((equality.right().clone(), outlives));
            }
        }
        None
    }
}

async fn reduce_type(
    ty: &Interned<Ty>,
    engine: &TrackedEngine,
    givens: &[crate::where_clause::PredicateKind],
) -> Option<(Interned<Ty>, OutlivesConstraints)> {
    match ty.as_ref() {
        Ty::Application(application) => {
            Box::pin(async move {
                if let ApplicationView::InstanceAssociated(associated) = application.view()
                    && let Some(reduced) =
                        crate::reduce::reduce_instance_associated(associated, engine).await
                    && reduced != *ty
                {
                    return Some((reduced, OutlivesConstraints::new()));
                }
                application.reduce(engine, givens).await.map(|(application, outlives)| {
                    (engine.intern(Ty::Application(application)), outlives)
                })
            })
            .await
        }
        // A lifetime never reduces.
        Ty::Inference(_) | Ty::PolyVar(_) | Ty::SelfInstance(_) | Ty::Lifetime(_) => None,
        Ty::EffectRow(row) => {
            if row.labels().len() == 0
                && let Some(tail) = row.tail()
            {
                return Some((tail.clone(), OutlivesConstraints::new()));
            }
            Box::pin(row.reduce(engine, givens))
                .await
                .map(|(row, outlives)| (engine.intern(Ty::EffectRow(row)), outlives))
        }
    }
}

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

impl Ty {
    pub async fn display<'x>(&'x self, engine: &TrackedEngine) -> TyDisplay<'x> {
        let mut poly_var_maps = FxHashMap::default();
        let mut symbol_names = FxHashMap::default();
        self.collect_display_context(engine, &mut poly_var_maps, &mut symbol_names).await;

        TyDisplay { ty: self, poly_var_maps, symbol_names }
    }

    async fn collect_display_context(
        &self,
        engine: &TrackedEngine,
        poly_var_maps: &mut FxHashMap<GlobalSymbolID, Interned<PolyVarMap>>,
        symbol_names: &mut FxHashMap<GlobalSymbolID, Interned<str>>,
    ) {
        for ty in self.recursive_iter() {
            match ty {
                Self::Application(application) => {
                    let symbol_id = match application.view() {
                        ApplicationView::Instance(instance) => Some(instance.symbol_id()),
                        ApplicationView::Struct(struct_) => Some(struct_.symbol_id()),
                        ApplicationView::InstanceAssociated(associated) => {
                            Some(associated.symbol_id())
                        }
                        ApplicationView::Primitive(_)
                        | ApplicationView::Tuple(_)
                        | ApplicationView::Pointer(_)
                        | ApplicationView::Reference(_)
                        | ApplicationView::DefInstance(_)
                        | ApplicationView::Closure(_)
                        | ApplicationView::NoOpDropInstance(_)
                        | ApplicationView::TupleDropInstance(_)
                        | ApplicationView::ClosureDropInstance(_)
                        | ApplicationView::NominalDropInstance(_)
                        | ApplicationView::Error => None,
                    };
                    if let Some(symbol_id) = symbol_id {
                        Self::collect_symbol_display_context(
                            engine,
                            symbol_id,
                            poly_var_maps,
                            symbol_names,
                        )
                        .await;
                    }
                }
                Self::Inference(_) | Self::SelfInstance(_) | Self::Lifetime(_) => {}
                Self::PolyVar(poly_var) => {
                    let symbol_id = poly_var.parent_id();
                    if let Entry::Vacant(entry) = poly_var_maps.entry(symbol_id) {
                        let poly_var_map = engine.query(&PolyVarKey { symbol_id }).await;
                        entry.insert(poly_var_map);
                    }
                }
                Self::EffectRow(row) => {
                    for label in row.labels() {
                        Self::collect_symbol_display_context(
                            engine,
                            label.effect_symbol_id(),
                            poly_var_maps,
                            symbol_names,
                        )
                        .await;
                    }
                }
            }
        }
    }

    async fn collect_symbol_display_context(
        engine: &TrackedEngine,
        symbol_id: GlobalSymbolID,
        poly_var_maps: &mut FxHashMap<GlobalSymbolID, Interned<PolyVarMap>>,
        symbol_names: &mut FxHashMap<GlobalSymbolID, Interned<str>>,
    ) {
        if let Entry::Vacant(entry) = poly_var_maps.entry(symbol_id) {
            entry.insert(engine.query(&PolyVarKey { symbol_id }).await);
        }
        if let Entry::Vacant(entry) = symbol_names.entry(symbol_id) {
            entry.insert(engine.get_name(symbol_id).await);
        }
    }
}

#[derive(Debug)]
pub struct TyDisplay<'x> {
    ty: &'x Ty,
    poly_var_maps: FxHashMap<GlobalSymbolID, Interned<PolyVarMap>>,
    symbol_names: FxHashMap<GlobalSymbolID, Interned<str>>,
}

impl TyDisplay<'_> {
    fn fmt_symbol_application<'x>(
        &self,
        symbol_id: GlobalSymbolID,
        arguments: impl IntoIterator<Item = &'x Interned<Ty>>,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        let name = self.symbol_names.get(&symbol_id).expect("should've been collected earlier");
        f.write_str(name)?;

        let poly_var_map =
            self.poly_var_maps.get(&symbol_id).expect("should've been collected earlier");
        let type_argument_count = poly_var_map
            .iter()
            .take_while(|(_, poly_var)| poly_var.kind() != TyKind::Instance)
            .count();

        let mut arguments = arguments.into_iter().peekable();
        if type_argument_count > 0 {
            f.write_char('[')?;
            for (index, argument) in arguments.by_ref().take(type_argument_count).enumerate() {
                if index > 0 {
                    f.write_str(", ")?;
                }
                self.fmt_ty(argument, f)?;
            }
            if arguments.peek().is_some() {
                f.write_str("; given ")?;
            }
        } else if arguments.peek().is_some() {
            f.write_str("[given ")?;
        } else {
            return Ok(());
        }

        for (index, argument) in arguments.enumerate() {
            if index > 0 {
                f.write_str(", ")?;
            }
            self.fmt_ty(argument, f)?;
        }
        f.write_char(']')?;

        Ok(())
    }

    fn fmt_signature(
        &self,
        parameters: &[Interned<Ty>],
        return_type: &Ty,
        effect_row: &Ty,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        f.write_char('(')?;
        for (index, parameter) in parameters.iter().enumerate() {
            if index > 0 {
                f.write_str(", ")?;
            }
            self.fmt_ty(parameter, f)?;
        }
        f.write_str(") -> ")?;
        self.fmt_ty(return_type, f)?;
        f.write_str(" \\ ")?;
        self.fmt_ty(effect_row, f)
    }

    /// Formats a built-in Drop dictionary by its kind and target type.
    fn fmt_drop_instance(
        &self,
        view: ApplicationView<'_>,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        let (description, target) = match view {
            ApplicationView::NoOpDropInstance(ty) => ("<no-op drop instance>", ty),
            ApplicationView::TupleDropInstance(instance) => {
                ("<tuple drop instance>", instance.tuple())
            }
            ApplicationView::ClosureDropInstance(instance) => {
                ("<closure drop instance>", instance.closure())
            }
            ApplicationView::NominalDropInstance(instance) => {
                ("<nominal drop instance>", instance.nominal())
            }
            ApplicationView::Primitive(_)
            | ApplicationView::Tuple(_)
            | ApplicationView::Pointer(_)
            | ApplicationView::Reference(_)
            | ApplicationView::Struct(_)
            | ApplicationView::Instance(_)
            | ApplicationView::InstanceAssociated(_)
            | ApplicationView::Closure(_)
            | ApplicationView::DefInstance(_)
            | ApplicationView::Error => panic!("expected a built-in Drop dictionary"),
        };
        f.write_str(description)?;
        self.fmt_ty(target, f)
    }

    fn fmt_reference(
        &self,
        reference: application::ReferenceView<'_>,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        f.write_char('&')?;

        // An erased lifetime is left out, as in Rust.
        if **reference.lifetime() != Ty::Lifetime(Lifetime::Erased) {
            self.fmt_ty(reference.lifetime(), f)?;
            f.write_char(' ')?;
        }
        if reference.mutability() == Mutability::Mutable {
            f.write_str("mut ")?;
        }
        self.fmt_ty(reference.pointee(), f)
    }

    fn fmt_ty(&self, ty: &Ty, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match ty {
            Ty::Application(ty_application) => match ty_application.view() {
                ApplicationView::Primitive(primitive) => match primitive {
                    Primitive::Int32 => write!(f, "int32"),
                    Primitive::Float32 => write!(f, "float32"),
                    Primitive::Bool => write!(f, "bool"),
                    Primitive::CInt => write!(f, "c_int"),
                    Primitive::CStr => write!(f, "cstr"),
                },

                view @ (ApplicationView::NoOpDropInstance(_)
                | ApplicationView::TupleDropInstance(_)
                | ApplicationView::ClosureDropInstance(_)
                | ApplicationView::NominalDropInstance(_)) => self.fmt_drop_instance(view, f),

                ApplicationView::Closure(closure) => {
                    f.write_str("<closure>")?;
                    self.fmt_signature(
                        closure.params(),
                        closure.return_type(),
                        closure.effect_row(),
                        f,
                    )
                }

                ApplicationView::Tuple(tuple) => {
                    f.write_char('(')?;

                    for (i, arg) in tuple.args().iter().enumerate() {
                        if i > 0 {
                            f.write_str(", ")?;
                        }
                        self.fmt_ty(arg, f)?;
                    }

                    f.write_char(')')
                }

                ApplicationView::Pointer(pointer) => {
                    f.write_char('*')?;
                    if pointer.mutability() == Mutability::Mutable {
                        f.write_str("mut ")?;
                    }
                    self.fmt_ty(pointer.pointee(), f)
                }
                ApplicationView::Reference(reference) => self.fmt_reference(reference, f),
                ApplicationView::Instance(instance) => {
                    self.fmt_symbol_application(instance.symbol_id(), instance.args(), f)
                }
                ApplicationView::Struct(struct_) => {
                    self.fmt_symbol_application(struct_.symbol_id(), struct_.args(), f)
                }
                ApplicationView::DefInstance(closure) => {
                    f.write_str("DefInstance[")?;
                    self.fmt_ty(closure, f)?;
                    f.write_char(']')
                }
                ApplicationView::InstanceAssociated(associated) => {
                    self.fmt_ty(associated.instance(), f)?;
                    f.write_char('.')?;
                    self.fmt_symbol_application(associated.symbol_id(), associated.args(), f)
                }
                ApplicationView::Error => write!(f, "<error>"),
            },

            Ty::Inference(inference) if inference.kind() == TyKind::Lifetime => f.write_str("'_"),
            Ty::Inference(inference) => match inference.constraint() {
                InferenceConstraint::Any => write!(f, "{{any}}"),
                InferenceConstraint::Numeric => {
                    write!(f, "{{numeric}}")
                }
                InferenceConstraint::EqualityComparable => {
                    write!(f, "{{equality comparable}}")
                }
            },

            Ty::SelfInstance(_) => f.write_str("this"),
            Ty::Lifetime(lifetime) => write!(f, "{lifetime}"),
            Ty::PolyVar(poly_var) => {
                let poly_var_map = self
                    .poly_var_maps
                    .get(&poly_var.parent_id())
                    .expect("should've been collected earlier");

                write!(f, "{}", poly_var_map.name_of(poly_var.id()))
            }

            Ty::EffectRow(row) => {
                f.write_char('{')?;

                for (index, label) in row.labels().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }

                    self.fmt_symbol_application(label.effect_symbol_id(), label.arguments(), f)?;
                }

                if let Some(tail) = row.tail() {
                    if row.labels().len() > 0 {
                        f.write_str(" | ")?;
                    } else {
                        f.write_str("| ")?;
                    }
                    self.fmt_ty(tail, f)?;
                }

                f.write_char('}')
            }
        }
    }
}

impl Display for TyDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { self.fmt_ty(self.ty, f) }
}

impl Ty {
    #[must_use]
    pub fn unwrap_as_application_view(&self) -> ApplicationView<'_> {
        let Self::Application(ty_application) = self else {
            panic!("Expected Ty::Application, found {self:?}");
        };

        ty_application.view()
    }

    #[must_use]
    pub fn as_tuple_view(&self) -> Option<application::TupleView<'_>> {
        let Self::Application(ty_application) = self else {
            return None;
        };

        let ApplicationView::Tuple(tuple_view) = ty_application.view() else {
            return None;
        };

        Some(tuple_view)
    }

    #[must_use]
    pub fn as_instance_view(&self) -> Option<InstanceView<'_>> {
        let Self::Application(ty_application) = self else {
            return None;
        };

        let ApplicationView::Instance(instance_view) = ty_application.view() else {
            return None;
        };

        Some(instance_view)
    }

    #[must_use]
    pub fn as_instance_associated_view(&self) -> Option<application::InstanceAssociatedView<'_>> {
        let Self::Application(ty_application) = self else {
            return None;
        };

        let ApplicationView::InstanceAssociated(view) = ty_application.view() else {
            return None;
        };

        Some(view)
    }

    /// Returns the type arguments of an application, or nothing for any
    /// other type.
    pub fn interned_arguments(ty: &Interned<Self>) -> impl Iterator<Item = &Interned<Self>> {
        let arguments = match &**ty {
            Self::Application(application) => Some(application.interned_iter()),
            Self::Inference(_)
            | Self::PolyVar(_)
            | Self::SelfInstance(_)
            | Self::EffectRow(_)
            | Self::Lifetime(_) => None,
        };
        arguments.into_iter().flatten()
    }

    /// Returns whether this lifetime takes part in named-lifetime outlives
    /// checks. Erased lifetimes are checked on the IR instead, and errors
    /// were already reported.
    #[must_use]
    pub fn is_checked_lifetime(&self) -> bool {
        *self != Self::Lifetime(Lifetime::Erased) && !self.contains_error()
    }

    #[must_use]
    pub fn as_struct_view(&self) -> Option<StructView<'_>> {
        let Self::Application(ty_application) = self else {
            return None;
        };

        let ApplicationView::Struct(struct_view) = ty_application.view() else {
            return None;
        };

        Some(struct_view)
    }

    #[must_use]
    pub const fn as_poly_var(&self) -> Option<&GlobalPolyVarID> {
        if let Self::PolyVar(poly_var) = self { Some(poly_var) } else { None }
    }

    /// Returns the region variable this type is, if it is one; see
    /// [`Lifetime::Region`].
    #[must_use]
    pub const fn as_region(&self) -> Option<RegionID> {
        if let Self::Lifetime(Lifetime::Region(region)) = self { Some(*region) } else { None }
    }

    #[must_use]
    pub fn open_closed_row(
        ty: &Interned<Self>,
        infer_gen: &mut impl GenInfer,
        engine: &TrackedEngine,
    ) -> Option<Interned<Self>> {
        if let Self::EffectRow(row) = ty.as_ref() {
            row.open_closed_row(infer_gen, engine).map(|x| engine.intern(Self::EffectRow(x)))
        } else {
            None
        }
    }

    #[must_use]
    pub const fn as_inference(&self) -> Option<&Inference> {
        if let Self::Inference(inference) = self { Some(inference) } else { None }
    }

    #[must_use]
    pub fn as_pointer_mutability(&self) -> Option<Mutability> {
        if let Self::Application(application) = self
            && let ApplicationView::Pointer(pointer) = application.view()
        {
            return Some(pointer.mutability());
        }

        None
    }

    #[must_use]
    pub fn as_reference_view(&self) -> Option<application::ReferenceView<'_>> {
        if let Self::Application(application) = self
            && let ApplicationView::Reference(reference) = application.view()
        {
            return Some(reference);
        }
        None
    }

    /// Returns what a dereference of this type reaches, when it is a raw
    /// pointer or a reference.
    #[must_use]
    pub fn as_dereferenceable(&self) -> Option<Dereferenceable<'_>> {
        let Self::Application(application) = self else {
            return None;
        };
        match application.view() {
            ApplicationView::Pointer(pointer) => Some(Dereferenceable {
                pointee: pointer.pointee(),
                mutability: pointer.mutability(),
                is_raw: true,
            }),
            ApplicationView::Reference(reference) => Some(Dereferenceable {
                pointee: reference.pointee(),
                mutability: reference.mutability(),
                is_raw: false,
            }),
            ApplicationView::Primitive(_)
            | ApplicationView::Tuple(_)
            | ApplicationView::Struct(_)
            | ApplicationView::Instance(_)
            | ApplicationView::InstanceAssociated(_)
            | ApplicationView::Closure(_)
            | ApplicationView::DefInstance(_)
            | ApplicationView::NoOpDropInstance(_)
            | ApplicationView::TupleDropInstance(_)
            | ApplicationView::ClosureDropInstance(_)
            | ApplicationView::NominalDropInstance(_)
            | ApplicationView::Error => None,
        }
    }

    #[must_use]
    pub fn as_pointee_of_pointer(&self) -> Option<&Interned<Self>> {
        if let Self::Application(application) = self
            && let ApplicationView::Pointer(pointer) = application.view()
        {
            return Some(pointer.pointee());
        }
        None
    }

    #[must_use]
    pub fn is_opaque_projection(&self) -> Option<bool> {
        match self {
            Self::Application(application) => match application.view() {
                ApplicationView::InstanceAssociated(associated) => Some(matches!(
                    &**associated.instance(),
                    Self::PolyVar(_) | Self::SelfInstance(_)
                )),
                ApplicationView::Error => None,
                ApplicationView::Primitive(_)
                | ApplicationView::Tuple(_)
                | ApplicationView::Pointer(_)
                | ApplicationView::Reference(_)
                | ApplicationView::Struct(_)
                | ApplicationView::Closure(_)
                | ApplicationView::DefInstance(_)
                | ApplicationView::NoOpDropInstance(_)
                | ApplicationView::TupleDropInstance(_)
                | ApplicationView::ClosureDropInstance(_)
                | ApplicationView::NominalDropInstance(_)
                | ApplicationView::Instance(_) => Some(false),
            },
            Self::Inference(_)
            | Self::PolyVar(_)
            | Self::SelfInstance(_)
            | Self::EffectRow(_)
            | Self::Lifetime(_) => Some(false),
        }
    }

    #[must_use]
    pub fn unwrap_as_closure_view(&self) -> application::ClosureView<'_> {
        self.as_closure_view().expect("expected a closure type")
    }

    #[must_use]
    pub fn as_closure_view(&self) -> Option<application::ClosureView<'_>> {
        let Self::Application(application) = self else {
            return None;
        };

        let ApplicationView::Closure(closure_view) = application.view() else {
            return None;
        };

        Some(closure_view)
    }

    #[must_use]
    pub fn is_c_abi_value_type(&self) -> bool {
        match self {
            Self::Application(application) => match application.view() {
                ApplicationView::Primitive(_) => true,
                ApplicationView::Pointer(pointer) => pointer.pointee().is_c_abi_value_type(),
                // A reference reaches C through a coercion to a raw pointer.
                ApplicationView::Reference(_)
                | ApplicationView::Tuple(_)
                | ApplicationView::Struct(_)
                | ApplicationView::InstanceAssociated(_)
                | ApplicationView::DefInstance(_)
                | ApplicationView::Instance(_)
                | ApplicationView::Closure(_)
                | ApplicationView::NoOpDropInstance(_)
                | ApplicationView::TupleDropInstance(_)
                | ApplicationView::ClosureDropInstance(_)
                | ApplicationView::NominalDropInstance(_)
                | ApplicationView::Error => false,
            },
            Self::EffectRow(_)
            | Self::Inference(_)
            | Self::PolyVar(_)
            | Self::SelfInstance(_)
            | Self::Lifetime(_) => false,
        }
    }

    #[must_use]
    pub fn is_unit_type(&self) -> bool {
        matches!(self, Self::Application(application) if matches!(application.view(), ApplicationView::Tuple(tuple) if tuple.args().is_empty()))
    }

    #[must_use]
    pub fn is_instance_associated(&self) -> bool {
        match self {
            Self::Application(application) => match application.view() {
                ApplicationView::InstanceAssociated(_) => true,
                ApplicationView::Primitive(_)
                | ApplicationView::Tuple(_)
                | ApplicationView::Pointer(_)
                | ApplicationView::Reference(_)
                | ApplicationView::Struct(_)
                | ApplicationView::DefInstance(_)
                | ApplicationView::Instance(_)
                | ApplicationView::Closure(_)
                | ApplicationView::NoOpDropInstance(_)
                | ApplicationView::TupleDropInstance(_)
                | ApplicationView::ClosureDropInstance(_)
                | ApplicationView::NominalDropInstance(_)
                | ApplicationView::Error => false,
            },
            Self::Inference(_)
            | Self::PolyVar(_)
            | Self::SelfInstance(_)
            | Self::EffectRow(_)
            | Self::Lifetime(_) => false,
        }
    }

    #[must_use]
    pub fn is_int32(&self) -> bool {
        matches!(self, Self::Application(application) if matches!(application.view(), ApplicationView::Primitive(Primitive::Int32)))
    }
}

/// What a dereference of a raw pointer or a reference reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dereferenceable<'x> {
    pointee: &'x Interned<Ty>,
    mutability: Mutability,
    is_raw: bool,
}

impl<'x> Dereferenceable<'x> {
    #[must_use]
    pub const fn pointee(&self) -> &'x Interned<Ty> { self.pointee }

    #[must_use]
    pub const fn mutability(&self) -> Mutability { self.mutability }

    /// Returns whether this is a raw pointer rather than a reference.
    #[must_use]
    pub const fn is_raw(&self) -> bool { self.is_raw }
}

#[cfg(test)]
mod test;
