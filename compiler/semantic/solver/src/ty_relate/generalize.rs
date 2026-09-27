//! Generalization of the type an inference variable is bound to in a
//! non-invariant relation.

use qbice::storage::intern::Interned;
use rayc_type::{
    ty::{Ty, TyKind, args::Args, effect_row::EffectLabel, inference::Inference},
    variance::Variance,
};

use super::Error;
use crate::solver::Solver;

impl Solver {
    /// Returns the generalization of `ty` for binding the inference variable
    /// `var` in a relation of variance `ambient`, following rustc.
    ///
    /// The type is walked with an ambient variance composed with each
    /// constructor's variance:
    ///
    /// - every inference variable in a non-invariant position becomes a fresh
    ///   inference variable of the same kind and constraint, while invariant
    ///   positions keep the original variable;
    /// - every lifetime, in any position, becomes a fresh lifetime inference
    ///   variable. Relating the generalization back to the original type never
    ///   binds it, and only produces outlives constraints.
    ///
    /// Fails the occurs check if `var` occurs in `ty`, in any position.
    pub(super) async fn generalize(
        &mut self,
        var: Inference,
        ty: &Interned<Ty>,
        ambient: Variance,
    ) -> Result<Interned<Ty>, Error> {
        Box::pin(async move {
            match &**ty {
                Ty::Inference(inference) => {
                    if *inference == var {
                        return Err(Error::OccursCheckFailed);
                    }

                    Ok(match ambient {
                        Variance::Invariant => ty.clone(),
                        Variance::Covariant | Variance::Contravariant | Variance::Bivariant => {
                            let fresh = self.new_inference_with_constraint(
                                inference.kind(),
                                inference.constraint(),
                            );
                            self.engine().intern(Ty::Inference(fresh))
                        }
                    })
                }

                Ty::Lifetime(_) => Ok(self.new_lifetime_inference()),

                Ty::PolyVar(_) => Ok(if ty.is_lifetime(self.engine()).await {
                    self.new_lifetime_inference()
                } else {
                    ty.clone()
                }),

                Ty::SelfInstance(_) => Ok(ty.clone()),

                Ty::Application(application) => {
                    // The arguments borrow the engine while `self` is borrowed
                    // mutably to generalize them.
                    let engine = self.engine().clone();
                    let mut arguments = Vec::new();
                    for (argument, variance) in
                        application.arguments_with_ambient_variance(ambient, &engine).await
                    {
                        arguments.push(self.generalize(var, argument, variance).await?);
                    }

                    let arguments = self.engine().intern_unsized(arguments);
                    Ok(self.engine().intern(Ty::Application(application.with_arguments(arguments))))
                }

                Ty::EffectRow(row) => {
                    let mut labels = Vec::with_capacity(row.labels().len());
                    for label in row.labels() {
                        labels.push(self.generalize_effect_label(var, label, ambient).await?);
                    }
                    let tail = match row.tail() {
                        Some(tail) => Some(self.generalize(var, tail, ambient).await?),
                        None => None,
                    };

                    Ok(Ty::new_effect_row(labels, tail, self.engine()))
                }
            }
        })
        .await
    }

    /// Creates a fresh lifetime inference variable.
    fn new_lifetime_inference(&mut self) -> Interned<Ty> {
        let fresh = self.new_inference(TyKind::Lifetime);
        self.engine().intern(Ty::Inference(fresh))
    }

    /// Generalizes the arguments of an effect label; see
    /// [`Self::generalize`].
    async fn generalize_effect_label(
        &mut self,
        var: Inference,
        label: &Interned<EffectLabel>,
        ambient: Variance,
    ) -> Result<Interned<EffectLabel>, Error> {
        // The arguments borrow the engine while `self` is borrowed mutably to
        // generalize them.
        let engine = self.engine().clone();
        let mut arguments = Vec::with_capacity(label.arguments().len());
        for (argument, variance) in label.arguments_with_ambient_variance(ambient, &engine).await {
            arguments.push(self.generalize(var, argument, variance).await?);
        }

        let arguments = Args::new(arguments, self.engine());
        Ok(self.engine().intern(EffectLabel::new(label.effect_symbol_id(), arguments)))
    }
}
