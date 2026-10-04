use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    constraint::{outlives::OutlivesConstraints, ty_relate::TyRelate},
    subst::Subst,
    ty::{Ty, application::Application, lifetime::Lifetime},
    variance::Variance,
};

use crate::solver::{Solver, TyRelatingEnvironment};

mod binding;
mod effect_row;
mod generalize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DerivationRule {
    TypeApplicationMatching,
    EffectLabelArgumentMatching {
        effect_symbol_id: GlobalSymbolID,
        argument_index: usize,
    },

    /// Relates the generalization that an inference variable was bound to
    /// back to the type it was generalized from; see [`Step::Generalized`].
    Generalization,
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
        variance: Variance,
    ) -> Self {
        Self::new(DerivationRule::TypeApplicationMatching, TyRelate::new(lesser, greater, variance))
    }

    #[must_use]
    pub const fn new_effect_label_argument_matching(
        effect_symbol_id: GlobalSymbolID,
        argument_index: usize,
        lesser: Interned<Ty>,
        greater: Interned<Ty>,
        variance: Variance,
    ) -> Self {
        Self::new(
            DerivationRule::EffectLabelArgumentMatching { effect_symbol_id, argument_index },
            TyRelate::new(lesser, greater, variance),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Step {
    /// A new substitution has been generated
    Subst(Subst),

    /// An inference variable was bound to a generalization of the other
    /// side. The substitution must be applied first; the derived constraints
    /// then relate the generalized type to the other side.
    Generalized { subst: Subst, derived: Vec<DerivedConstraint> },

    /// The constraint has been simplified to a set of new constraints
    Derived(Vec<DerivedConstraint>),

    /// No applicable rules could be found
    NoProgress,
}

/// The result of one relation step: what the step did, and the outlives
/// constraints it requires.
///
/// The constraints come from relating two lifetimes, which never binds
/// anything, and from normalizing the related types through given equalities
/// that match modulo lifetimes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entailment {
    step: Step,
    outlives: OutlivesConstraints,
}

impl Entailment {
    /// Creates the result of a step that requires no outlives constraint.
    #[must_use]
    pub fn new(step: Step) -> Self { Self::with_outlives(step, OutlivesConstraints::new()) }

    #[must_use]
    pub const fn with_outlives(step: Step, outlives: OutlivesConstraints) -> Self {
        Self { step, outlives }
    }

    #[must_use]
    pub const fn step(&self) -> &Step { &self.step }

    #[must_use]
    pub const fn outlives(&self) -> &OutlivesConstraints { &self.outlives }

    #[must_use]
    pub fn into_parts(self) -> (Step, OutlivesConstraints) { (self.step, self.outlives) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
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
        (TyRelatingEnvironment::Normal, _, VariableKind::Poly | VariableKind::External) => false,
        (TyRelatingEnvironment::TopLevelMatching, TyRelatingSide::Lesser, VariableKind::Poly) => {
            true
        }
        (TyRelatingEnvironment::TopLevelMatching, _, _) => false,
        (
            TyRelatingEnvironment::InterfaceMatching,
            TyRelatingSide::Lesser,
            VariableKind::External,
        ) => true,
        (TyRelatingEnvironment::InterfaceMatching, _, _) => false,
    }
}

enum VariableKind {
    Inference,
    Poly,

    /// An external lifetime of a nested IR function.
    External,
}

impl VariableKind {
    /// Returns the kind of variable that matching binds the lifetime `ty`
    /// as, if it is one that some matching binds: a lifetime parameter or an
    /// external lifetime.
    const fn of_matched_lifetime(ty: &Ty) -> Option<Self> {
        match ty {
            Ty::PolyVar(_) => Some(Self::Poly),
            Ty::Lifetime(Lifetime::External(_)) => Some(Self::External),
            Ty::Lifetime(Lifetime::Static | Lifetime::Erased | Lifetime::Region(_))
            | Ty::Application(_)
            | Ty::Inference(_)
            | Ty::SelfInstance(_)
            | Ty::EffectRow(_) => None,
        }
    }
}

#[derive(Clone, Copy)]
enum TyRelatingSide {
    Lesser,
    Greater,
}

impl TyRelatingSide {
    /// Returns the relation between a type on this side and one on the
    /// other side.
    const fn relate(
        self,
        this_side: Interned<Ty>,
        other_side: Interned<Ty>,
        variance: Variance,
    ) -> TyRelate {
        match self {
            Self::Lesser => TyRelate::new(this_side, other_side, variance),
            Self::Greater => TyRelate::new(other_side, this_side, variance),
        }
    }
}

impl Solver {
    pub async fn entail_ty_relate(&mut self, ty_relate: &TyRelate) -> Result<Entailment, Error> {
        self.entail_ty_relate_with(ty_relate, &TyRelatingEnvironment::Normal).await
    }

    /// Takes one step towards solving `relate`.
    ///
    /// Both sides are normalized first, so a projection that the step meets
    /// is irreducible under the current bindings. A relation that makes no
    /// progress may still make progress once a variable in it is bound.
    pub async fn entail_ty_relate_with(
        &mut self,
        relate: &TyRelate,
        relate_env: &TyRelatingEnvironment,
    ) -> Result<Entailment, Error> {
        // Matching relates instance and marker heads, whose arguments are
        // invariant, or an interface to the very types it is created with.
        // Binding a matched variable therefore never needs generalization, and
        // everything derived stays invariant.
        assert!(
            !relate_env.is_matching() || relate.variance() == Variance::Invariant,
            "matching must relate types invariantly"
        );

        let (relate, normalization_outlives) = self.normalize_with_outlives(relate).await;
        let (step, outlives) =
            self.entail_normalized_ty_relate(&relate, relate_env).await?.into_parts();

        Ok(Entailment::with_outlives(step, normalization_outlives.union(outlives)))
    }

    /// Takes one step towards solving a normalized `relate`.
    async fn entail_normalized_ty_relate(
        &mut self,
        relate: &TyRelate,
        relate_env: &TyRelatingEnvironment,
    ) -> Result<Entailment, Error> {
        if relate.lesser() == relate.greater() {
            return Ok(Entailment::new(Step::Derived(Vec::new())));
        }

        // Relating two lifetimes never binds anything: it only produces
        // outlives constraints, even under invariance.
        if self.is_outlives_relation(relate, relate_env).await {
            let outlives = OutlivesConstraints::from_relation(
                relate.lesser(),
                relate.greater(),
                relate.variance(),
            );
            return Ok(Entailment::with_outlives(Step::Derived(Vec::new()), outlives));
        }

        let variance = relate.variance();
        let step = match (&**relate.lesser(), &**relate.greater()) {
            (Ty::Application(lesser), Ty::Application(greater)) => {
                match (lesser.is_instance_associated(), greater.is_instance_associated()) {
                    // Two irreducible projections are only known to be equal
                    // when they are equal modulo lifetimes. Their arguments
                    // are invariant, so each pair of lifetimes is related
                    // invariantly, which may bind a matched one.
                    (true, true) => Ok(Ty::corresponding_lifetimes(
                        relate.lesser(),
                        relate.greater(),
                        self.engine(),
                    )
                    .await
                    .map_or(Step::NoProgress, |lifetimes| {
                        Step::Derived(
                            lifetimes
                                .into_iter()
                                .map(|(lesser, greater)| {
                                    DerivedConstraint::new_type_application_matching(
                                        lesser,
                                        greater,
                                        Variance::Invariant,
                                    )
                                })
                                .collect(),
                        )
                    })),

                    // An irreducible projection can still reduce once a
                    // variable in it is bound, to a type that satisfies the
                    // relation.
                    (true, false) | (false, true) => Ok(Step::NoProgress),

                    (false, false) => self.decompose_applications(lesser, greater, variance).await,
                }
            }

            // Effect rows use exact Koka-style row unification: labels are
            // neither deduplicated nor accepted through subeffect inclusion,
            // and open rows are rewritten to a shared tail. Only the
            // lifetimes in matched labels are related by the variance.
            (Ty::EffectRow(lesser), Ty::EffectRow(greater)) => {
                self.entail_effect_row_relate(lesser, greater, variance).await.map(Step::Derived)
            }

            // A head variable that top-level matching may bind is bound first,
            // even to an inference variable, such as a lifetime inference
            // variable in the goal.
            (Ty::PolyVar(poly_var), _)
                if can_bind(relate_env, TyRelatingSide::Lesser, VariableKind::Poly) =>
            {
                self.bind_poly_var(*poly_var, relate.greater(), TyRelatingSide::Lesser, relate_env)
                    .await
            }

            // An external lifetime that interface matching may bind is
            // instantiated with the lifetime the creator has in its place.
            (Ty::Lifetime(Lifetime::External(external)), _)
                if can_bind(relate_env, TyRelatingSide::Lesser, VariableKind::External) =>
            {
                self.bind_external_lifetime(*external, relate.greater()).await
            }

            (Ty::Inference(var), _) => {
                self.bind_infer_var(
                    *var,
                    relate.greater(),
                    TyRelatingSide::Lesser,
                    relate_env,
                    variance,
                )
                .await
            }
            (_, Ty::Inference(var)) => {
                self.bind_infer_var(
                    *var,
                    relate.lesser(),
                    TyRelatingSide::Greater,
                    relate_env,
                    variance,
                )
                .await
            }

            _ => {
                if relate.lesser().is_instance_associated()
                    || relate.greater().is_instance_associated()
                {
                    Ok(Step::NoProgress)
                } else {
                    Err(Error::Conflicted)
                }
            }
        }?;

        Ok(Entailment::new(step))
    }

    /// Returns whether `relate` relates two lifetimes by outlives
    /// constraints.
    ///
    /// A lifetime that matching may bind is instantiated through substitution
    /// instead: a lifetime parameter of an instance head, or an external
    /// lifetime of an interface. That is ordinary instantiation, not
    /// inference. Lifetime inference variables are never bound, not even
    /// under invariance.
    async fn is_outlives_relation(
        &self,
        relate: &TyRelate,
        relate_env: &TyRelatingEnvironment,
    ) -> bool {
        if !relate.lesser().is_lifetime(self.engine()).await
            || !relate.greater().is_lifetime(self.engine()).await
        {
            return false;
        }

        let is_matched = VariableKind::of_matched_lifetime(relate.lesser())
            .is_some_and(|kind| can_bind(relate_env, TyRelatingSide::Lesser, kind));
        !is_matched
    }

    /// Relates two applications of the same type constructor argument by
    /// argument, each with the variance of its position.
    async fn decompose_applications(
        &self,
        lesser: &Application,
        greater: &Application,
        variance: Variance,
    ) -> Result<Step, Error> {
        let Some(arguments) = lesser.structural_match(greater) else {
            return Err(Error::Conflicted);
        };

        let variances = lesser.arguments_with_ambient_variance(variance, self.engine()).await;

        Ok(Step::Derived(
            arguments
                .zip(variances)
                .map(|((lesser, greater), (_, variance))| {
                    DerivedConstraint::new_type_application_matching(
                        lesser.clone(),
                        greater.clone(),
                        variance,
                    )
                })
                .collect(),
        ))
    }
}

#[cfg(test)]
mod test;
