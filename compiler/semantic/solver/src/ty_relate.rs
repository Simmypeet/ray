use qbice::storage::intern::Interned;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    constraint::ty_relate::TyRelate,
    poly_var::{GlobalPolyVarID, get_poly_var_map},
    subst::Subst,
    ty::{
        InferenceConstraint, Ty, TyKind,
        effect_row::{EffectLabel, EffectRow},
        inference::Inference,
    },
};

use crate::solver::{Solver, TyRelatingEnvironment};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DerivationRule {
    TypeApplicationMatching,
    EffectLabelArgumentMatching { effect_symbol_id: GlobalSymbolID, argument_index: usize },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DerivedConstraint {
    pub rule: DerivationRule,
    pub ty_relate: TyRelate,
}

impl DerivedConstraint {
    #[must_use]
    pub const fn new(rule: DerivationRule, ty_relate: TyRelate) -> Self { Self { rule, ty_relate } }

    #[must_use]
    pub const fn new_type_application_matching(
        lesser: Interned<Ty>,
        greater: Interned<Ty>,
    ) -> Self {
        Self::new(DerivationRule::TypeApplicationMatching, TyRelate::new(lesser, greater))
    }

    #[must_use]
    pub const fn new_effect_label_argument_matching(
        effect_symbol_id: GlobalSymbolID,
        argument_index: usize,
        lesser: Interned<Ty>,
        greater: Interned<Ty>,
    ) -> Self {
        Self::new(
            DerivationRule::EffectLabelArgumentMatching { effect_symbol_id, argument_index },
            TyRelate::new(lesser, greater),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Step {
    /// A new substitution has been generated
    Subst(Subst),

    /// The constraint has been simplified to a set of new constraints
    Derived(Vec<DerivedConstraint>),

    /// No applicable rules could be found
    NoProgress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Error {
    /// The subtype constraint is obviously unsatisfiable, e.g. `Int <: Bool`
    Conflicted,

    /// The subtype constraint is unsatisfiable due to a cycle, e.g. `T <: T`
    OccursCheckFailed,
}

#[expect(clippy::trivially_copy_pass_by_ref)]
const fn can_bind(
    environment: &TyRelatingEnvironment,
    side: TyRelatingSide,
    var_kind: VariableKind,
) -> bool {
    #[expect(clippy::match_same_arms)]
    match (environment, side, var_kind) {
        (TyRelatingEnvironment::Normal, _, VariableKind::Inference) => true,
        (TyRelatingEnvironment::Normal, _, VariableKind::Poly) => false,
        (TyRelatingEnvironment::TopLevelMatching, TyRelatingSide::Lesser, VariableKind::Poly) => {
            true
        }
        (TyRelatingEnvironment::TopLevelMatching, _, _) => false,
    }
}

enum VariableKind {
    Inference,
    Poly,
}

enum TyRelatingSide {
    Lesser,
    Greater,
}

impl Solver {
    pub async fn entail_ty_relate(&mut self, ty_relate: &TyRelate) -> Result<Step, Error> {
        self.entail_ty_relate_with(ty_relate, &TyRelatingEnvironment::Normal).await
    }

    pub async fn entail_ty_relate_with(
        &mut self,
        substype: &TyRelate,
        relate_env: &TyRelatingEnvironment,
    ) -> Result<Step, Error> {
        if substype.lesser() == substype.greater() {
            return Ok(Step::Derived(Vec::new()));
        }

        match (&**substype.lesser(), &**substype.greater()) {
            (Ty::Application(l1), Ty::Application(l2)) => l1.structural_match(l2).map_or_else(
                || Err(Error::Conflicted),
                |arg| {
                    Ok(Step::Derived(
                        arg.map(|(l, g)| {
                            DerivedConstraint::new_type_application_matching(l.clone(), g.clone())
                        })
                        .collect(),
                    ))
                },
            ),

            // Effect rows use exact Koka-style row unification here. Despite the
            // enclosing `Subtype` name, this is equality: labels are neither
            // deduplicated nor accepted through subeffect inclusion, and open
            // rows are rewritten to a shared tail.
            (Ty::EffectRow(lesser), Ty::EffectRow(greater)) => {
                self.entail_effect_row_subtype(lesser, greater).map(Step::Derived)
            }

            (Ty::Inference(var), _) => Ok(Step::Subst(
                self.bind_infer_var(*var, substype.greater(), TyRelatingSide::Lesser, relate_env)
                    .await?,
            )),
            (_, Ty::Inference(var)) => Ok(Step::Subst(
                self.bind_infer_var(*var, substype.lesser(), TyRelatingSide::Greater, relate_env)
                    .await?,
            )),

            (Ty::PolyVar(poly_var), _) => Ok(Step::Subst(
                self.bind_poly_var(
                    *poly_var,
                    substype.greater(),
                    TyRelatingSide::Lesser,
                    relate_env,
                )
                .await?,
            )),
            (_, Ty::PolyVar(poly_var)) => Ok(Step::Subst(
                self.bind_poly_var(
                    *poly_var,
                    substype.lesser(),
                    TyRelatingSide::Greater,
                    relate_env,
                )
                .await?,
            )),

            _ => Err(Error::Conflicted),
        }
    }

    fn entail_effect_row_subtype(
        &mut self,
        lesser: &EffectRow,
        greater: &EffectRow,
    ) -> Result<Vec<DerivedConstraint>, Error> {
        let MatchedEffectRowLabels { mut constraints, unmatched_lesser, unmatched_greater } =
            match_effect_row_labels(lesser, greater)?;

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
                constraints.push(DerivedConstraint::new_type_application_matching(
                    lesser_tail.clone(),
                    greater_remainder,
                ));
            }
            (None, Some(greater_tail)) => {
                if !unmatched_greater.is_empty() {
                    return Err(Error::Conflicted);
                }
                let lesser_remainder = Ty::new_effect_row(unmatched_lesser, None, self.engine());
                constraints.push(DerivedConstraint::new_type_application_matching(
                    lesser_remainder,
                    greater_tail.clone(),
                ));
            }
            (Some(lesser_tail), Some(greater_tail)) => {
                if lesser_tail == greater_tail {
                    if !unmatched_lesser.is_empty() || !unmatched_greater.is_empty() {
                        return Err(Error::Conflicted);
                    }
                } else if unmatched_lesser.is_empty() && unmatched_greater.is_empty() {
                    constraints.push(match_effect_row_tails(lesser_tail, greater_tail));
                } else if unmatched_lesser.is_empty() {
                    let greater_remainder = Ty::new_effect_row(
                        unmatched_greater,
                        Some(greater_tail.clone()),
                        self.engine(),
                    );
                    constraints.push(DerivedConstraint::new_type_application_matching(
                        lesser_tail.clone(),
                        greater_remainder,
                    ));
                } else if unmatched_greater.is_empty() {
                    let lesser_remainder = Ty::new_effect_row(
                        unmatched_lesser,
                        Some(lesser_tail.clone()),
                        self.engine(),
                    );
                    constraints.push(DerivedConstraint::new_type_application_matching(
                        lesser_remainder,
                        greater_tail.clone(),
                    ));
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
                        DerivedConstraint::new_type_application_matching(
                            lesser_tail.clone(),
                            greater_remainder,
                        ),
                        DerivedConstraint::new_type_application_matching(
                            lesser_remainder,
                            greater_tail.clone(),
                        ),
                    ]);
                }
            }
        }

        Ok(constraints)
    }

    async fn bind_poly_var(
        &mut self,
        poly_var: GlobalPolyVarID,
        ty: &Interned<Ty>,
        relating_side: TyRelatingSide,
        relate_env: &TyRelatingEnvironment,
    ) -> Result<Subst, Error> {
        if !can_bind(relate_env, relating_side, VariableKind::Poly) {
            return Err(Error::Conflicted);
        }

        if ty.has_poly_variable(&poly_var) {
            return Err(Error::OccursCheckFailed);
        }

        let poly_var_map = self.engine().get_poly_var_map(poly_var.parent_id()).await;
        let kind = poly_var_map[poly_var.id()].kind();

        if kind != ty.kind_of(self.engine()).await {
            return Err(Error::Conflicted);
        }

        Ok(Subst::new_singleton(poly_var, ty.clone()))
    }

    async fn bind_infer_var(
        &mut self,
        var: Inference,
        ty: &Interned<Ty>,
        relating_side: TyRelatingSide,
        relate_env: &TyRelatingEnvironment,
    ) -> Result<Subst, Error> {
        if !can_bind(relate_env, relating_side, VariableKind::Inference) {
            return Err(Error::Conflicted);
        }

        if ty.has_inference_variable(&var) {
            return Err(Error::OccursCheckFailed);
        }

        if var.kind() != ty.kind_of(self.engine()).await {
            return Err(Error::Conflicted);
        }

        match &**ty {
            Ty::Application(ty_application) => {
                if var.kind() == TyKind::Star
                    && !ty_application.satisfies_constraint(var.constraint())
                {
                    return Err(Error::Conflicted);
                }

                Ok(Subst::new_singleton(var, ty.clone()))
            }

            Ty::Inference(ty_inference) => {
                if var.constraint() == ty_inference.constraint() {
                    return Ok(Subst::new_singleton(var, ty.clone()));
                }

                let meet =
                    var.constraint().meet(&ty_inference.constraint()).ok_or(Error::Conflicted)?;

                let common_var = self.new_inference_with_constraint(var.kind(), meet);
                let common_var = self.engine().intern(Ty::Inference(common_var));

                Ok([(var, common_var.clone()), (*ty_inference, common_var)].into_iter().collect())
            }

            Ty::PolyVar(_) | Ty::EffectRow(_) => {
                if var.constraint() == InferenceConstraint::Any {
                    Ok(Subst::new_singleton(var, ty.clone()))
                } else {
                    Err(Error::Conflicted)
                }
            }
        }
    }
}

struct MatchedEffectRowLabels {
    constraints: Vec<DerivedConstraint>,
    unmatched_lesser: Vec<Interned<EffectLabel>>,
    unmatched_greater: Vec<Interned<EffectLabel>>,
}

fn match_effect_row_labels(
    lesser: &EffectRow,
    greater: &EffectRow,
) -> Result<MatchedEffectRowLabels, Error> {
    let mut constraints = Vec::new();
    let mut unmatched_lesser = Vec::new();
    let mut unmatched_greater = greater.labels().cloned().collect::<Vec<_>>();

    for lesser_label in lesser.labels() {
        let matching_effect = unmatched_greater.iter().position(|greater_label| {
            greater_label.effect_symbol_id() == lesser_label.effect_symbol_id()
        });

        let Some(matching_effect) = matching_effect else {
            unmatched_lesser.push(lesser_label.clone());
            continue;
        };

        let greater_label = unmatched_greater.remove(matching_effect);
        let Some(arguments) = lesser_label.structural_match(&greater_label) else {
            return Err(Error::Conflicted);
        };
        constraints.extend(arguments.enumerate().map(|(argument_index, (lesser, greater))| {
            DerivedConstraint::new_effect_label_argument_matching(
                lesser_label.effect_symbol_id(),
                argument_index,
                lesser.clone(),
                greater.clone(),
            )
        }));
    }

    Ok(MatchedEffectRowLabels { constraints, unmatched_lesser, unmatched_greater })
}

fn match_effect_row_tails(lesser: &Interned<Ty>, greater: &Interned<Ty>) -> DerivedConstraint {
    DerivedConstraint::new_type_application_matching(lesser.clone(), greater.clone())
}

#[cfg(test)]
mod test;
