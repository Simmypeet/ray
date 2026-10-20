use qbice::storage::intern::Interned;
use rayc_qbice::TrackedEngine;

use crate::{
    subst::{Subst, Substitutable},
    ty::{Ty, lifetime::Lifetime},
};

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
