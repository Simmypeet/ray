use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_qbice::TrackedEngine;

use crate::{
    constraint::{Constraint, Error, Step},
    solver::Solver,
    subst::{Subst, Substitutable},
    ty::{Ty, TyInference},
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Subtype {
    lesser: Interned<Ty>,
    greater: Interned<Ty>,
}

impl Subtype {
    #[must_use]
    pub const fn lesser(&self) -> &Interned<Ty> { &self.lesser }

    #[must_use]
    pub const fn greater(&self) -> &Interned<Ty> { &self.greater }
}

impl Substitutable for Subtype {
    fn apply_subst(&self, subst: &Subst, engine: &TrackedEngine) -> Option<Self>
    where
        Self: Sized,
    {
        match (self.lesser.apply_subst(subst, engine), self.greater.apply_subst(subst, engine)) {
            (Some(lesser), Some(greater)) => Some(Self { lesser, greater }),
            (Some(lesser), None) => Some(Self { lesser, greater: self.greater.clone() }),
            (None, Some(greater)) => Some(Self { lesser: self.lesser.clone(), greater }),
            (None, None) => None,
        }
    }
}

impl Subtype {
    #[must_use]
    pub const fn new(lesser: Interned<Ty>, greater: Interned<Ty>) -> Self {
        Self { lesser, greater }
    }
}

impl Solver {
    #[allow(clippy::unused_self)]
    pub(super) fn entail_subtype(&mut self, substype: &Subtype) -> Result<Step, Error> {
        if substype.lesser == substype.greater {
            return Ok(Step::Simplified(Vec::new()));
        }

        match (&*substype.lesser, &*substype.greater) {
            (Ty::Application(l1), Ty::Application(l2)) => l1.structural_match(l2).map_or_else(
                || Err(Error::Conflicted),
                |arg| {
                    Ok(Step::Simplified(
                        arg.map(|(l, g)| Constraint::Subtype(Subtype::new(l.clone(), g.clone())))
                            .collect(),
                    ))
                },
            ),

            (Ty::Inference(var), _) => Ok(Step::Subst(self.bind_var(*var, &substype.greater)?)),

            (_, Ty::Inference(var)) => Ok(Step::Subst(self.bind_var(*var, &substype.lesser)?)),
        }
    }

    fn bind_var(&mut self, var: TyInference, ty: &Interned<Ty>) -> Result<Subst, Error> {
        if ty.has_inference_variable(&var) {
            return Err(Error::OccursCheckFailed);
        }

        match &**ty {
            Ty::Application(ty_application) => {
                if !ty_application.satisfies_constraint(var.constraint()) {
                    return Err(Error::Conflicted);
                }

                Ok(Subst::new_singleton(var, ty.clone()))
            }

            Ty::Inference(ty_inference) => {
                if var.constraint() == ty_inference.constraint() {
                    return Ok(Subst::new_singleton(var, ty.clone()));
                }

                assert_eq!(var.kind(), ty_inference.kind(), "ill-kinded type!");
                let meet =
                    var.constraint().meet(&ty_inference.constraint()).ok_or(Error::Conflicted)?;

                let common_var = self.new_inference_with_constraint(var.kind(), meet);
                let common_var = self.engine().intern(Ty::Inference(common_var));

                Ok([(var, common_var.clone()), (*ty_inference, common_var)].into_iter().collect())
            }
        }
    }
}

#[cfg(test)]
mod test;
