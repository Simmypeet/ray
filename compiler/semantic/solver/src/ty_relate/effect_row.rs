//! Koka-style unification of effect rows, with the lifetimes in matched
//! labels related by each effect's variances.

use qbice::storage::intern::Interned;
use rayc_type::{
    constraint::{outlives::OutlivesConstraints, ty_relate::TyRelate},
    ty::{
        Ty, TyKind,
        effect_row::{EffectLabel, EffectRow},
    },
    variance::Variance,
};

use super::{DerivedConstraint, Error};
use crate::solver::Solver;

impl Solver {
    /// Returns the outlives constraints that let something whose effect is
    /// `introduced`, such as a call, run where the effect is `ambient`,
    /// without binding any variables.
    ///
    /// Type checking opens a closed `introduced` row before unifying it with
    /// `ambient`, so its labels only have to be a part of those of
    /// `ambient`: each is related to the label it matches, covariantly, and
    /// the labels and the tail of `ambient` that are left stand for the tail
    /// the row was opened with. Any other `introduced` effect is related to
    /// the whole of `ambient`.
    ///
    /// Returns `None` if the relation fails or could only hold by binding a
    /// variable; see [`Self::solve_without_unify`].
    pub async fn relate_introduced_effect_without_unify(
        &mut self,
        introduced: &Interned<Ty>,
        ambient: &Interned<Ty>,
    ) -> Option<OutlivesConstraints> {
        // A row that a substitution or a projection put in the tail of
        // another is flattened first, so that every label is matched.
        let (introduced, introduced_outlives) = self.normalize_with_outlives(introduced).await;
        let (ambient, ambient_outlives) = self.normalize_with_outlives(ambient).await;

        let constraints = if let (Ty::EffectRow(introduced_row), Ty::EffectRow(ambient_row)) =
            (&*introduced, &*ambient)
            && introduced_row.tail().is_none()
        {
            let labels = self
                .match_effect_row_labels(introduced_row, ambient_row, Variance::Covariant)
                .await
                .ok()?;

            // The tail the row was opened with only stands for labels of
            // `ambient`.
            if !labels.unmatched_lesser.is_empty() {
                return None;
            }

            labels.constraints.into_iter().map(|derived| derived.ty_relate).collect()
        } else {
            vec![TyRelate::new(introduced.clone(), ambient.clone(), Variance::Covariant)]
        };

        let outlives = self.solve_without_unify(constraints).await?;
        Some(introduced_outlives.union(ambient_outlives).union(outlives))
    }

    pub(super) async fn entail_effect_row_relate(
        &mut self,
        lesser: &EffectRow,
        greater: &EffectRow,
        variance: Variance,
    ) -> Result<Vec<DerivedConstraint>, Error> {
        let MatchedEffectRowLabels { mut constraints, unmatched_lesser, unmatched_greater } =
            self.match_effect_row_labels(lesser, greater, variance).await?;

        // The tails, and the remainders rewritten onto them, carry the
        // variance of the rows.
        let relate_tails = |lesser: Interned<Ty>, greater: Interned<Ty>| {
            DerivedConstraint::new_type_application_matching(lesser, greater, variance)
        };

        match (lesser.tail(), greater.tail()) {
            (None, None) => {
                if !unmatched_lesser.is_empty() || !unmatched_greater.is_empty() {
                    return Err(Error::Conflicted);
                }
            }
            (Some(lesser_tail), None) => {
                if !unmatched_lesser.is_empty() {
                    return Err(Error::Conflicted);
                }
                let greater_remainder = Ty::new_effect_row(unmatched_greater, None, self.engine());
                constraints.push(relate_tails(lesser_tail.clone(), greater_remainder));
            }
            (None, Some(greater_tail)) => {
                if !unmatched_greater.is_empty() {
                    return Err(Error::Conflicted);
                }
                let lesser_remainder = Ty::new_effect_row(unmatched_lesser, None, self.engine());
                constraints.push(relate_tails(lesser_remainder, greater_tail.clone()));
            }
            (Some(lesser_tail), Some(greater_tail)) => {
                if lesser_tail == greater_tail {
                    if !unmatched_lesser.is_empty() || !unmatched_greater.is_empty() {
                        return Err(Error::Conflicted);
                    }
                } else if unmatched_lesser.is_empty() && unmatched_greater.is_empty() {
                    constraints.push(relate_tails(lesser_tail.clone(), greater_tail.clone()));
                } else if unmatched_lesser.is_empty() {
                    let greater_remainder = Ty::new_effect_row(
                        unmatched_greater,
                        Some(greater_tail.clone()),
                        self.engine(),
                    );
                    constraints.push(relate_tails(lesser_tail.clone(), greater_remainder));
                } else if unmatched_greater.is_empty() {
                    let lesser_remainder = Ty::new_effect_row(
                        unmatched_lesser,
                        Some(lesser_tail.clone()),
                        self.engine(),
                    );
                    constraints.push(relate_tails(lesser_remainder, greater_tail.clone()));
                } else {
                    let common_tail = self.new_inference(TyKind::EffectRow);
                    let common_tail = self.engine().intern(Ty::Inference(common_tail));
                    let greater_remainder = Ty::new_effect_row(
                        unmatched_greater,
                        Some(common_tail.clone()),
                        self.engine(),
                    );
                    let lesser_remainder =
                        Ty::new_effect_row(unmatched_lesser, Some(common_tail), self.engine());
                    constraints.extend([
                        relate_tails(lesser_tail.clone(), greater_remainder),
                        relate_tails(lesser_remainder, greater_tail.clone()),
                    ]);
                }
            }
        }

        Ok(constraints)
    }

    /// Matches the labels of two rows by effect symbol. Matched labels must
    /// agree argument by argument, and each pair of arguments is related by
    /// the variance of its effect parameter inside `variance`.
    async fn match_effect_row_labels(
        &self,
        lesser: &EffectRow,
        greater: &EffectRow,
        variance: Variance,
    ) -> Result<MatchedEffectRowLabels, Error> {
        let labels = lesser.match_labels(greater);

        let mut constraints = Vec::new();
        for (lesser_label, greater_label) in labels.matched() {
            let effect_symbol_id = lesser_label.effect_symbol_id();
            let Some(arguments) = lesser_label.structural_match(greater_label) else {
                return Err(Error::Conflicted);
            };

            let variances =
                lesser_label.arguments_with_ambient_variance(variance, self.engine()).await;

            constraints.extend(arguments.zip(variances).enumerate().map(
                |(argument_index, ((lesser, greater), (_, variance)))| {
                    DerivedConstraint::new_effect_label_argument_matching(
                        effect_symbol_id,
                        argument_index,
                        lesser.clone(),
                        greater.clone(),
                        variance,
                    )
                },
            ));
        }

        Ok(MatchedEffectRowLabels {
            constraints,
            unmatched_lesser: labels.unmatched_left().cloned().collect(),
            unmatched_greater: labels.unmatched_right().cloned().collect(),
        })
    }
}

struct MatchedEffectRowLabels {
    constraints: Vec<DerivedConstraint>,
    unmatched_lesser: Vec<Interned<EffectLabel>>,
    unmatched_greater: Vec<Interned<EffectLabel>>,
}
