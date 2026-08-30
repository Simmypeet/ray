use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_qbice::TrackedEngine;
use rayc_type::{
    subst::{MutSubstitutable, Subst, Substitutable},
    ty::Ty,
};

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub struct TypedThunkContext {
    return_type: Interned<Ty>,
}

impl TypedThunkContext {
    pub(crate) const fn new(return_type: Interned<Ty>) -> Self { Self { return_type } }

    #[must_use]
    pub const fn return_type(&self) -> &Interned<Ty> { &self.return_type }
}

impl MutSubstitutable for TypedThunkContext {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        self.return_type.apply_in_place(subst, engine);
    }
}
