use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};

use crate::{
    reduce::Reduce,
    subst::Substitutable,
    ty::{Ty, inference::Inference},
};

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct Args {
    args: Interned<[Interned<Ty>]>,
}

impl Args {
    #[must_use]
    pub fn new(
        args: impl IntoIterator<Item = Interned<Ty>>,
        engine: &rayc_qbice::TrackedEngine,
    ) -> Self {
        Self { args: engine.intern_unsized(args.into_iter().collect::<Vec<_>>()) }
    }

    pub fn interned_iter(&self) -> impl Iterator<Item = &Interned<Ty>> { self.args.iter() }

    pub fn iter(&self) -> impl Iterator<Item = &Ty> {
        self.args.iter().map(std::convert::AsRef::as_ref)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool { self.args.is_empty() }

    #[must_use]
    pub fn structural_match<'a>(
        &'a self,
        other: &'a Self,
    ) -> Option<impl Iterator<Item = (&'a Interned<Ty>, &'a Interned<Ty>)>> {
        (self.args.len() == other.args.len()).then(|| self.args.iter().zip(other.args.iter()))
    }

    #[must_use]
    pub fn has_inference_variable(&self, ty: &Inference) -> bool {
        self.args.iter().any(|x| x.has_inference_variable(ty))
    }
}

impl Reduce for Args {
    fn reduce(&self, engine: &rayc_qbice::TrackedEngine) -> Option<Self> {
        self.args.reduce(engine).map(|args| Self { args })
    }
}

impl Substitutable for Args {
    fn apply_subst(
        &self,
        subst: &crate::subst::Subst,
        engine: &rayc_qbice::TrackedEngine,
    ) -> Option<Self>
    where
        Self: Sized,
    {
        self.args.apply_subst(subst, engine).map(|args| Self { args })
    }
}
