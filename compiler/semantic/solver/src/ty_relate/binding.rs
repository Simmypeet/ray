//! Binding inference and polymorphic variables.

use qbice::storage::intern::Interned;
use rayc_type::{
    poly_var::{GlobalPolyVarID, get_poly_var_map},
    subst::Subst,
    ty::{InferenceConstraint, Ty, TyKind, inference::Inference},
    variance::Variance,
};

use super::{
    DerivationRule, DerivedConstraint, Error, Step, TyRelatingSide, VariableKind, can_bind,
};
use crate::solver::{Solver, TyRelatingEnvironment};

impl Solver {
    /// Binds a polymorphic variable, which only top-level matching allows.
    ///
    /// Top-level matching relates instance and marker heads, whose arguments
    /// are invariant, so the variable is bound to the type itself without
    /// generalization.
    pub(super) async fn bind_poly_var(
        &mut self,
        poly_var: GlobalPolyVarID,
        ty: &Interned<Ty>,
        relating_side: TyRelatingSide,
        relate_env: &TyRelatingEnvironment,
    ) -> Result<Step, Error> {
        if !can_bind(relate_env, relating_side, VariableKind::Poly) {
            return if ty.is_instance_associated() {
                Ok(Step::NoProgress)
            } else {
                Err(Error::Conflicted)
            };
        }

        if ty.has_poly_variable(&poly_var) {
            return Err(Error::OccursCheckFailed);
        }

        let poly_var_map = self.engine().get_poly_var_map(poly_var.parent_id()).await;
        let kind = poly_var_map[poly_var.id()].kind();

        if kind != ty.kind_of(self.engine()).await {
            return Err(Error::Conflicted);
        }

        Ok(Step::Subst(Subst::new_singleton(poly_var, ty.clone())))
    }

    /// Binds an inference variable to `ty`.
    ///
    /// In a non-invariant relation, the variable is bound to a generalization
    /// of `ty` instead, which is then related back to `ty`; see
    /// [`Self::generalize`].
    pub(super) async fn bind_infer_var(
        &mut self,
        var: Inference,
        ty: &Interned<Ty>,
        relating_side: TyRelatingSide,
        relate_env: &TyRelatingEnvironment,
        variance: Variance,
    ) -> Result<Step, Error> {
        if !can_bind(relate_env, relating_side, VariableKind::Inference) {
            // technically, the instance associated can be reduced into something that can
            // be equal to this inference variable and thus discharging the
            // constraint
            return if ty.is_instance_associated() {
                Ok(Step::NoProgress)
            } else {
                Err(Error::Conflicted)
            };
        }

        if var.kind() != ty.kind_of(self.engine()).await {
            return Err(Error::Conflicted);
        }

        // Check that the variable's constraint admits the head of `ty`. A
        // generalization keeps the head, so this holds for it too.
        match &**ty {
            Ty::Application(ty_application) => {
                if var.kind() == TyKind::Star
                    && !ty_application.satisfies_constraint(var.constraint())
                {
                    // associated type could reduce to a type that satisfies the constraint in the
                    // future, don't make it a conflict yet
                    return if ty_application.is_instance_associated() {
                        Ok(Step::NoProgress)
                    } else {
                        Err(Error::Conflicted)
                    };
                }
            }

            Ty::Inference(ty_inference) => {
                return self.bind_infer_vars(var, *ty_inference, ty, variance);
            }

            Ty::PolyVar(_) | Ty::SelfInstance(_) | Ty::EffectRow(_) | Ty::Lifetime(_) => {
                if var.constraint() != InferenceConstraint::Any {
                    return Err(Error::Conflicted);
                }
            }
        }

        // Bind the variable, generalizing `ty` first in a non-invariant
        // relation. Generalization also runs the occurs check.
        let generalized = match variance {
            Variance::Invariant => {
                if ty.has_inference_variable(&var) {
                    return Err(Error::OccursCheckFailed);
                }

                return Ok(Step::Subst(Subst::new_singleton(var, ty.clone())));
            }
            Variance::Covariant | Variance::Contravariant | Variance::Bivariant => {
                self.generalize(var, ty, variance).await?
            }
        };

        let subst = Subst::new_singleton(var, generalized.clone());

        // Relate the generalization back to `ty` from the variable's side.
        let relate = relating_side.relate(generalized, ty.clone(), variance);
        Ok(Step::Generalized {
            subst,
            derived: vec![DerivedConstraint::new(DerivationRule::Generalization, relate)],
        })
    }

    /// Unifies two distinct inference variables, meeting their constraints.
    ///
    /// Unifying makes the types the variables stand for identical, lifetimes
    /// included, which only an invariant relation allows. In any other
    /// relation, the variables are unified only when one of them can only
    /// stand for a primitive type, which has no lifetimes: a numeric or
    /// equality-comparable literal. Otherwise, the relation waits until one
    /// variable is bound, so the other one can be bound to a generalization of
    /// it.
    fn bind_infer_vars(
        &mut self,
        var: Inference,
        other: Inference,
        other_ty: &Interned<Ty>,
        variance: Variance,
    ) -> Result<Step, Error> {
        let is_lifetime_free = |inference: Inference| match inference.constraint() {
            InferenceConstraint::Numeric | InferenceConstraint::EqualityComparable => true,
            InferenceConstraint::Any => false,
        };
        let may_unify = match variance {
            Variance::Invariant => true,
            Variance::Covariant | Variance::Contravariant | Variance::Bivariant => {
                is_lifetime_free(var) || is_lifetime_free(other)
            }
        };
        if !may_unify {
            return Ok(Step::NoProgress);
        }

        if var.constraint() == other.constraint() {
            return Ok(Step::Subst(Subst::new_singleton(var, other_ty.clone())));
        }

        let meet = var.constraint().meet(&other.constraint()).ok_or(Error::Conflicted)?;

        let common_var = self.new_inference_with_constraint(var.kind(), meet);
        let common_var = self.engine().intern(Ty::Inference(common_var));

        Ok(Step::Subst([(var, common_var.clone()), (other, common_var)].into_iter().collect()))
    }
}
