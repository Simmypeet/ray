use std::collections::VecDeque;

use qbice::storage::intern::Interned;
use rayc_qbice::TrackedEngine;

use crate::{
    constraint::outlives::OutlivesConstraints,
    poly_var::{GlobalPolyVarID, get_poly_var_map},
    rewrite::{RewriteAsync, TyRewriterAsync},
    ty::{
        Ty, TyKind, application::View as ApplicationView, inference::Inference, lifetime::Lifetime,
    },
    variance::Variance,
    where_clause::OutlivesPredicate,
};

impl Ty {
    /// Returns whether this is a built-in `Drop` dictionary that does nothing.
    ///
    /// Tuple and closure dictionaries are no-ops when all their element or
    /// capture dictionaries are recursively no-ops, including empty ones.
    ///
    /// A coroutine dictionary never is, whatever its upvar dictionaries are:
    /// dropping a suspended coroutine drops its saved locals, which its type
    /// does not show.
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
                | ApplicationView::Enum(_)
                | ApplicationView::Instance(_)
                | ApplicationView::InstanceAssociated(_)
                | ApplicationView::Closure(_)
                | ApplicationView::DefInstance(_)
                | ApplicationView::DefCoroutine(_)
                | ApplicationView::TraitDefCoroutine(_)
                | ApplicationView::ContinuationDefInstance(_)
                | ApplicationView::CoroutineInstance(_)
                | ApplicationView::CoroutineDropInstance(_)
                | ApplicationView::GeneratedAdtDropInstance(_)
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
                | ApplicationView::Enum(_)
                | ApplicationView::InstanceAssociated(_)
                | ApplicationView::Closure(_)
                | ApplicationView::DefInstance(_)
                | ApplicationView::NoOpDropInstance(_)
                | ApplicationView::TupleDropInstance(_)
                | ApplicationView::ClosureDropInstance(_)
                | ApplicationView::DefCoroutine(_)
                | ApplicationView::TraitDefCoroutine(_)
                | ApplicationView::ContinuationDefInstance(_)
                | ApplicationView::CoroutineInstance(_)
                | ApplicationView::CoroutineDropInstance(_)
                | ApplicationView::GeneratedAdtDropInstance(_)
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

    /// Returns whether this type, which the caller knows to be of kind
    /// [`TyKind::Lifetime`], is a universal lifetime: `'static`, a lifetime
    /// parameter or an external lifetime, a region which the function
    /// mentioning it does not choose, but is given; see
    /// [`Lifetime::is_universal`].
    ///
    /// A parameter of kind lifetime is a lifetime parameter, so its kind is
    /// not looked up. For a type of another kind, the answer is meaningless:
    /// check [`Self::is_lifetime`] first when the kind is not known.
    #[must_use]
    pub const fn is_universal_region(&self) -> bool {
        match self {
            Self::Lifetime(lifetime) => lifetime.is_universal(),
            Self::PolyVar(_) => true,
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
        let lifetimes = Self::corresponding_lifetimes(left, right, engine).await?;

        Some(
            lifetimes
                .iter()
                .flat_map(|(left, right)| {
                    OutlivesPredicate::from_relation(left, right, Variance::Invariant)
                })
                .collect(),
        )
    }

    /// Pairs each lifetime of `left` with the lifetime at the same position
    /// in `right`, when the two types are equal up to their lifetimes; see
    /// [`Self::equal_modulo_lifetimes`].
    ///
    /// Returns `None` if they are not equal. Otherwise, returns the pairs
    /// whose lifetimes differ, in unspecified order, the lifetime of `left`
    /// first. A position where both types have the same lifetime has no
    /// pair.
    pub async fn corresponding_lifetimes(
        left: &Interned<Self>,
        right: &Interned<Self>,
        engine: &TrackedEngine,
    ) -> Option<Vec<(Interned<Self>, Interned<Self>)>> {
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

        Some(lifetimes)
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
/// because the kind of a type is queried from the engine.
struct LifetimeEraser<'e> {
    engine: &'e TrackedEngine,
}

impl TyRewriterAsync for LifetimeEraser<'_> {
    async fn rewrite(&mut self, ty: &Interned<Ty>) -> Option<Interned<Ty>> {
        // An erased lifetime is kept as it is, so that a type with nothing
        // left to erase is not rebuilt.
        if ty.is_lifetime(self.engine).await && **ty != Ty::Lifetime(Lifetime::Erased) {
            Some(Ty::new_lifetime(Lifetime::Erased, self.engine))
        } else {
            None
        }
    }
}
