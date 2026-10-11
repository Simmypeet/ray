use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_qbice::TrackedEngine;
use rayc_type::{
    subst::{MutSubstitutable, Subst, Substitutable},
    ty::{Ty, application::CoroutineView},
};

use crate::typed_expr::{SubExprs, TypedExprID};

/// Suspends a call before it runs: `coro f(args)`.
///
/// The arguments are evaluated and stored in a new coroutine, and none of the
/// callee runs. The value is a continuation that refers to the coroutine, so
/// the type of the expression is not the coroutine type itself.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Coro {
    /// The type of the coroutine created, which names the suspended function
    /// and instantiates its polymorphic parameters.
    coroutine_ty: Interned<Ty>,
    arguments: Vec<TypedExprID>,
}

impl Coro {
    /// # Panics
    ///
    /// If `coroutine_ty` is not a coroutine type.
    #[must_use]
    pub fn new(coroutine_ty: Interned<Ty>, arguments: Vec<TypedExprID>) -> Self {
        assert!(coroutine_ty.as_coroutine_view().is_some(), "`coro` creates a coroutine type");
        Self { coroutine_ty, arguments }
    }

    /// Returns the type of the coroutine created.
    #[must_use]
    pub const fn coroutine_ty(&self) -> &Interned<Ty> { &self.coroutine_ty }

    /// Returns the coroutine type as the function it suspends and the
    /// arguments it instantiates that function with.
    #[must_use]
    pub fn coroutine(&self) -> CoroutineView<'_> {
        self.coroutine_ty.as_coroutine_view().expect("`coro` creates a coroutine type")
    }

    #[must_use]
    pub fn arguments(&self) -> &[TypedExprID] { &self.arguments }
}

impl MutSubstitutable for Coro {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        self.coroutine_ty.apply_in_place(subst, engine);
    }
}

impl SubExprs for Coro {
    fn sub_exprs(&self) -> impl Iterator<Item = TypedExprID> { self.arguments.iter().copied() }
}
