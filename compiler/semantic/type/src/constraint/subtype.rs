use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_qbice::TrackedEngine;

use crate::{
    constraint::{Constraint, Error, Step},
    solver::Solver,
    subst::{Subst, Substitutable},
    ty::{Ty, TyInference, TyKind, effect_row::EffectRow},
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

            (Ty::EffectRow(lesser), Ty::EffectRow(greater)) => {
                self.entail_effect_row_subtype(lesser, greater)
            }

            (Ty::Inference(var), _) => Ok(Step::Subst(self.bind_var(*var, &substype.greater)?)),

            (_, Ty::Inference(var)) => Ok(Step::Subst(self.bind_var(*var, &substype.lesser)?)),

            (Ty::Application(_), Ty::PolyVar(_) | Ty::EffectRow(_))
            | (Ty::PolyVar(_), Ty::Application(_) | Ty::PolyVar(_) | Ty::EffectRow(_))
            | (Ty::EffectRow(_), Ty::Application(_) | Ty::PolyVar(_)) => Err(Error::Conflicted),
        }
    }

    fn entail_effect_row_subtype(
        &mut self,
        lesser: &EffectRow,
        greater: &EffectRow,
    ) -> Result<Step, Error> {
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
            constraints.extend(arguments.map(|(lesser, greater)| {
                Constraint::Subtype(Subtype::new(lesser.clone(), greater.clone()))
            }));
        }

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
                constraints.push(Constraint::Subtype(Subtype::new(
                    lesser_tail.clone(),
                    greater_remainder,
                )));
            }
            (None, Some(greater_tail)) => {
                if !unmatched_greater.is_empty() {
                    return Err(Error::Conflicted);
                }
                let lesser_remainder = Ty::new_effect_row(unmatched_lesser, None, self.engine());
                constraints.push(Constraint::Subtype(Subtype::new(
                    lesser_remainder,
                    greater_tail.clone(),
                )));
            }
            (Some(lesser_tail), Some(greater_tail)) => {
                if lesser_tail == greater_tail {
                    if !unmatched_lesser.is_empty() || !unmatched_greater.is_empty() {
                        return Err(Error::Conflicted);
                    }
                } else if unmatched_lesser.is_empty() {
                    let greater_remainder = Ty::new_effect_row(
                        unmatched_greater,
                        Some(greater_tail.clone()),
                        self.engine(),
                    );
                    constraints.push(Constraint::Subtype(Subtype::new(
                        lesser_tail.clone(),
                        greater_remainder,
                    )));
                } else if unmatched_greater.is_empty() {
                    let lesser_remainder = Ty::new_effect_row(
                        unmatched_lesser,
                        Some(lesser_tail.clone()),
                        self.engine(),
                    );
                    constraints.push(Constraint::Subtype(Subtype::new(
                        lesser_remainder,
                        greater_tail.clone(),
                    )));
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
                        Constraint::Subtype(Subtype::new(lesser_tail.clone(), greater_remainder)),
                        Constraint::Subtype(Subtype::new(lesser_remainder, greater_tail.clone())),
                    ]);
                }
            }
        }

        Ok(Step::Simplified(constraints))
    }

    fn bind_var(&mut self, var: TyInference, ty: &Interned<Ty>) -> Result<Subst, Error> {
        if ty.has_inference_variable(&var) {
            return Err(Error::OccursCheckFailed);
        }

        match &**ty {
            Ty::Application(ty_application) => {
                if var.kind() != TyKind::Star
                    || !ty_application.satisfies_constraint(var.constraint())
                {
                    return Err(Error::Conflicted);
                }

                Ok(Subst::new_singleton(var, ty.clone()))
            }

            Ty::Inference(ty_inference) => {
                if var.kind() != ty_inference.kind() {
                    return Err(Error::Conflicted);
                }

                if var.constraint() == ty_inference.constraint() {
                    return Ok(Subst::new_singleton(var, ty.clone()));
                }

                let meet =
                    var.constraint().meet(&ty_inference.constraint()).ok_or(Error::Conflicted)?;

                let common_var = self.new_inference_with_constraint(var.kind(), meet);
                let common_var = self.engine().intern(Ty::Inference(common_var));

                Ok([(var, common_var.clone()), (*ty_inference, common_var)].into_iter().collect())
            }

            Ty::PolyVar(_) => {
                if var.constraint() == crate::ty::InferenceConstraint::Any {
                    Ok(Subst::new_singleton(var, ty.clone()))
                } else {
                    Err(Error::Conflicted)
                }
            }

            Ty::EffectRow(_) => {
                if var.kind() == TyKind::EffectRow
                    && var.constraint() == crate::ty::InferenceConstraint::Any
                {
                    Ok(Subst::new_singleton(var, ty.clone()))
                } else {
                    Err(Error::Conflicted)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use qbice::storage::intern::Interned;
    use rayc_qbice::TrackedEngine;
    use rayc_symbol::SymbolID;
    use rayc_target::TargetID;

    use super::Subtype;
    use crate::{
        constraint::{Constraint, Error, Step},
        solver::Solver,
        subst::{Subst, Substitutable},
        ty::{Primitive, Ty, TyInference, TyKind, args::Args, effect_row::EffectLabel},
    };

    fn effect_label(id: u128, engine: &TrackedEngine) -> Interned<EffectLabel> {
        effect_label_with_args(id, [], engine)
    }

    fn effect_label_with_args(
        id: u128,
        args: impl IntoIterator<Item = Interned<Ty>>,
        engine: &TrackedEngine,
    ) -> Interned<EffectLabel> {
        let symbol_id = TargetID::TEST.make_global(SymbolID::from_u128(id));
        engine.intern(EffectLabel::new(symbol_id, Args::new(args, engine)))
    }

    fn solve(
        solver: &mut Solver,
        constraint: Constraint,
        engine: &TrackedEngine,
    ) -> Result<Subst, Error> {
        let mut pending = vec![constraint];
        let mut subst = Subst::new_empty();

        while let Some(constraint) = pending.pop() {
            let constraint = constraint.apply_subst_or_clone(&subst, engine);
            match solver.entail(&constraint)? {
                Step::Subst(new_subst) => subst.compose(&new_subst, engine),
                Step::Simplified(constraints) => pending.extend(constraints),
                Step::NoProgress => panic!("effect-row constraint should make progress"),
            }
        }

        Ok(subst)
    }

    // input: {IO, Exn} <: {Exn, IO}
    // premise: different effect constructors commute
    // output: {}
    #[tokio::test]
    async fn closed_effect_rows_match_independent_of_label_order() {
        let engine = rayc_qbice::create_minimal_engine().await;
        let io = effect_label(1, &engine);
        let exn = effect_label(2, &engine);
        let lesser = Ty::new_effect_row([io.clone(), exn.clone()], None, &engine);
        let greater = Ty::new_effect_row([exn, io], None, &engine);
        let mut solver = Solver::new(engine);

        let step = solver.entail(&Constraint::Subtype(Subtype::new(lesser, greater)));

        assert_eq!(step, Ok(Step::Simplified(Vec::new())));
    }

    // input: {Exn, Exn} <: {Exn}
    // premise: duplicate effect labels are significant
    // output: Conflicted
    #[tokio::test]
    async fn closed_effect_rows_preserve_duplicate_labels() {
        let engine = rayc_qbice::create_minimal_engine().await;
        let exn = effect_label(1, &engine);
        let lesser = Ty::new_effect_row([exn.clone(), exn.clone()], None, &engine);
        let greater = Ty::new_effect_row([exn], None, &engine);
        let mut solver = Solver::new(engine);

        let step = solver.entail(&Constraint::Subtype(Subtype::new(lesser, greater)));

        assert_eq!(step, Err(Error::Conflicted));
    }

    // input: {IO | e1} <: {State | e2}
    // premise: e1 and e2 are distinct open effect-row variables
    // output: e1 <: {State | e3}, {IO | e3} <: e2
    #[tokio::test]
    async fn open_effect_rows_share_a_fresh_common_tail() {
        let engine = rayc_qbice::create_minimal_engine().await;
        let io = effect_label(1, &engine);
        let state = effect_label(2, &engine);
        let mut solver = Solver::new(engine.clone());
        let e1 = engine.intern(Ty::Inference(solver.new_inference(TyKind::EffectRow)));
        let e2 = engine.intern(Ty::Inference(solver.new_inference(TyKind::EffectRow)));
        let lesser = Ty::new_effect_row([io.clone()], Some(e1.clone()), &engine);
        let greater = Ty::new_effect_row([state.clone()], Some(e2.clone()), &engine);

        let step = solver.entail(&Constraint::Subtype(Subtype::new(lesser, greater)));

        let e3 = engine.intern(Ty::Inference(TyInference::new(TyKind::EffectRow, 2)));
        let state_remainder = Ty::new_effect_row([state], Some(e3.clone()), &engine);
        let io_remainder = Ty::new_effect_row([io], Some(e3), &engine);
        assert_eq!(
            step,
            Ok(Step::Simplified(vec![
                Constraint::Subtype(Subtype::new(e1, state_remainder)),
                Constraint::Subtype(Subtype::new(io_remainder, e2)),
            ]))
        );
    }

    // input: e <: {IO}
    // premise: e is an effect-row inference variable
    // output: e := {IO}
    #[tokio::test]
    async fn effect_row_inference_binds_to_an_effect_row() {
        let engine = rayc_qbice::create_minimal_engine().await;
        let io = effect_label(1, &engine);
        let mut solver = Solver::new(engine.clone());
        let inference = solver.new_inference(TyKind::EffectRow);
        let inference_ty = engine.intern(Ty::Inference(inference));
        let row = Ty::new_effect_row([io], None, &engine);

        let step = solver.entail(&Constraint::Subtype(Subtype::new(inference_ty, row.clone())));

        assert_eq!(step, Ok(Step::Subst(crate::subst::Subst::new_singleton(inference, row))));
    }

    // input: {State[int32], State[bool]} = {State[bool], State[int32]}
    // premise: occurrences of the same effect constructor cannot commute
    // output: Conflicted
    #[tokio::test]
    async fn same_effect_constructor_occurrences_cannot_swap() {
        let engine = rayc_qbice::create_minimal_engine().await;
        let int32 = Ty::new_primitive(Primitive::Int32, &engine);
        let bool = Ty::new_primitive(Primitive::Bool, &engine);
        let state_int32 = effect_label_with_args(1, [int32], &engine);
        let state_bool = effect_label_with_args(1, [bool], &engine);
        let lesser = Ty::new_effect_row([state_int32.clone(), state_bool.clone()], None, &engine);
        let greater = Ty::new_effect_row([state_bool, state_int32], None, &engine);
        let mut solver = Solver::new(engine.clone());

        let result =
            solve(&mut solver, Constraint::Subtype(Subtype::new(lesser, greater)), &engine);

        assert_eq!(result, Err(Error::Conflicted));
    }

    // input: {State[?a], State[?b]} = {State[int32], State[bool]}
    // premise: same-constructor occurrences match in row order
    // output: ?a := int32, ?b := bool
    #[tokio::test]
    async fn same_effect_constructor_inferences_bind_in_occurrence_order() {
        let engine = rayc_qbice::create_minimal_engine().await;
        let int32 = Ty::new_primitive(Primitive::Int32, &engine);
        let bool = Ty::new_primitive(Primitive::Bool, &engine);
        let mut solver = Solver::new(engine.clone());
        let a = solver.new_inference(TyKind::Star);
        let b = solver.new_inference(TyKind::Star);
        let a_ty = engine.intern(Ty::Inference(a));
        let b_ty = engine.intern(Ty::Inference(b));
        let state_a = effect_label_with_args(1, [a_ty], &engine);
        let state_b = effect_label_with_args(1, [b_ty], &engine);
        let state_int32 = effect_label_with_args(1, [int32.clone()], &engine);
        let state_bool = effect_label_with_args(1, [bool.clone()], &engine);
        let lesser = Ty::new_effect_row([state_a, state_b], None, &engine);
        let greater = Ty::new_effect_row([state_int32, state_bool], None, &engine);

        let subst = solve(&mut solver, Constraint::Subtype(Subtype::new(lesser, greater)), &engine)
            .expect("same-constructor occurrences should match positionally");

        assert_eq!(subst.get(&a), Some(&int32));
        assert_eq!(subst.get(&b), Some(&bool));
    }
}
