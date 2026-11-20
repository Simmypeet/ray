use crate::{
    subst::Var,
    ty::{Ty, lifetime::Lifetime},
};

impl Ty {
    /// Returns the variable this type is, which a substitution may replace.
    pub(crate) const fn as_var(&self) -> Option<Var> {
        match self {
            Self::Inference(inference) => Some(Var::Inference(*inference)),
            Self::PolyVar(poly) => Some(Var::Poly(*poly)),
            Self::SelfInstance(instance) => Some(Var::SelfInstance(*instance)),
            Self::Lifetime(Lifetime::External(external)) => Some(Var::External(*external)),

            Self::Lifetime(Lifetime::Static | Lifetime::Erased | Lifetime::Region(_))
            | Self::Application(_)
            | Self::EffectRow(_)
            | Self::EffectRow2(_) => None,
        }
    }
}
