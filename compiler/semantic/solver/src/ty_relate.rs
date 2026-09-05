use qbice::storage::intern::Interned;
use rayc_type::{
    constraint::{DerivedConstraint, Error, Step, ty_relate::TyRelate},
    poly_var::{GlobalPolyVarID, get_poly_var_map},
    subst::Subst,
    ty::{
        InferenceConstraint, Ty, TyKind,
        effect_row::{EffectLabel, EffectRow},
        inference::Inference,
    },
};

use crate::solver::{Solver, TyRelatingEnvironment};

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
    pub(super) async fn entail_subtype(
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
mod tests {
    use std::{collections::HashMap, sync::Arc};

    use qbice::storage::intern::Interned;
    use rayc_lexical::tree::{OffsetMode, RelativeLocation, RelativeSpan};
    use rayc_qbice::{Engine, InMemoryFactory, PrecomputedExecutor, TrackedEngine};
    use rayc_symbol::SymbolID;
    use rayc_target::TargetID;
    use rayc_type::{
        constraint::{Constraint, DerivedConstraint, Error, Step},
        poly_var::{GlobalPolyVarID, PolyVar, PolyVarMap},
        subst::{Subst, Substitutable},
        ty::{Primitive, Ty, TyKind, args::Args, effect_row::EffectLabel, inference::Inference},
    };

    use super::{Solver, TyRelate};

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

    async fn engine_with_effect_poly_var() -> (TrackedEngine, GlobalPolyVarID) {
        let mut engine = Engine::new_with(
            qbice::serialize::Plugin::default(),
            InMemoryFactory,
            qbice::stable_hash::SeededStableHasherBuilder::new(0),
        )
        .await
        .unwrap();
        let parent_id = TargetID::TEST.make_global(SymbolID::from_u128(0));
        let location = RelativeLocation {
            offset: 0,
            mode: OffsetMode::Start,
            relative_to: rayc_arena::ID::new(0),
        };
        let mut poly_vars = PolyVarMap::new();
        let id = poly_vars
            .insert(PolyVar::new_effect(engine.intern_unsized("p"), RelativeSpan {
                start: location,
                end: location,
                source_id: TargetID::TEST.make_global(rayc_source_file::LocalSourceID::new(0, 0)),
            }))
            .unwrap();
        engine.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
            rayc_type::poly_var::Key { symbol_id: parent_id },
            engine.intern(poly_vars),
        )]))));
        (Arc::new(engine).tracked().await, GlobalPolyVarID::new(parent_id, id))
    }

    // input: ?dict = Instance[int32]
    // premise: ?dict has kind Instance; equality may put it on either side
    // output: exactly ?dict := Instance[int32]
    #[tokio::test]
    async fn instance_inference_binds_to_concrete_instance() {
        let engine = rayc_qbice::create_minimal_engine().await;
        let symbol = TargetID::TEST.make_global(SymbolID::from_u128(1));
        let instance = Ty::new_instance(
            symbol,
            Args::new([Ty::new_primitive(Primitive::Int32, &engine)], &engine),
            &engine,
        );
        for reverse in [false, true] {
            let mut solver = Solver::new(engine.clone());
            let inference = solver.new_inference(TyKind::Instance);
            let variable = engine.intern(Ty::Inference(inference));
            let (left, right) =
                if reverse { (instance.clone(), variable) } else { (variable, instance.clone()) };
            assert_eq!(
                solver.entail(&Constraint::TyRelate(TyRelate::new(left, right))).await,
                Ok(Step::Subst(Subst::new_singleton(inference, instance.clone())))
            );
        }
    }

    // input: ?k = Error(k)
    // premise: ?k is unconstrained and k is Star, Instance, or EffectRow
    // output: exactly ?k := Error(k)
    #[tokio::test]
    async fn inference_binds_to_error_of_its_kind() {
        let engine = rayc_qbice::create_minimal_engine().await;
        for kind in [TyKind::Star, TyKind::Instance, TyKind::EffectRow] {
            let mut solver = Solver::new(engine.clone());
            let inference = solver.new_inference(kind);
            let variable = engine.intern(Ty::Inference(inference));
            let error = Ty::new_error(kind, &engine);
            assert_eq!(
                solver.entail(&Constraint::TyRelate(TyRelate::new(variable, error.clone()))).await,
                Ok(Step::Subst(Subst::new_singleton(inference, error)))
            );
        }
    }

    // input: ?k = int32, Instance[], {}, or Error(j)
    // premise: k differs from the concrete type's kind, in either equality
    // direction output: Conflicted
    #[tokio::test]
    async fn inference_rejects_cross_kind_bindings() {
        let engine = rayc_qbice::create_minimal_engine().await;
        let symbol = TargetID::TEST.make_global(SymbolID::from_u128(1));
        let kinds = [TyKind::Star, TyKind::Instance, TyKind::EffectRow];
        let concrete = [
            (TyKind::Star, Ty::new_primitive(Primitive::Int32, &engine)),
            (TyKind::Instance, Ty::new_instance(symbol, Args::new([], &engine), &engine)),
            (TyKind::EffectRow, Ty::new_effect_row([], None, &engine)),
        ];
        for (kind, ty) in
            concrete.into_iter().chain(kinds.map(|kind| (kind, Ty::new_error(kind, &engine))))
        {
            for inference_kind in kinds.into_iter().filter(|other| *other != kind) {
                for reverse in [false, true] {
                    let mut solver = Solver::new(engine.clone());
                    let inference = solver.new_inference(inference_kind);
                    let variable = engine.intern(Ty::Inference(inference));
                    let (left, right) =
                        if reverse { (ty.clone(), variable) } else { (variable, ty.clone()) };
                    assert_eq!(
                        solver.entail(&Constraint::TyRelate(TyRelate::new(left, right))).await,
                        Err(Error::Conflicted)
                    );
                }
            }
        }
    }

    // input: ?numeric or ?equality = a primitive, tuple, or star error
    // premise: numeric accepts numbers; equality also accepts bool
    // output: a singleton substitution for allowed primitives, otherwise Conflicted
    #[tokio::test]
    async fn star_application_constraints_remain_enforced() {
        use rayc_type::ty::InferenceConstraint;

        let engine = rayc_qbice::create_minimal_engine().await;
        for (constraint, primitive, allowed) in [
            (InferenceConstraint::Numeric, Primitive::Int32, true),
            (InferenceConstraint::Numeric, Primitive::Float32, true),
            (InferenceConstraint::Numeric, Primitive::CInt, true),
            (InferenceConstraint::Numeric, Primitive::Bool, false),
            (InferenceConstraint::Numeric, Primitive::CStr, false),
            (InferenceConstraint::EqualityComparable, Primitive::Bool, true),
            (InferenceConstraint::EqualityComparable, Primitive::CStr, false),
        ] {
            let mut solver = Solver::new(engine.clone());
            let inference = solver.new_inference_with_constraint(TyKind::Star, constraint);
            let variable = engine.intern(Ty::Inference(inference));
            let ty = Ty::new_primitive(primitive, &engine);
            let expected = if allowed {
                Ok(Step::Subst(Subst::new_singleton(inference, ty.clone())))
            } else {
                Err(Error::Conflicted)
            };
            assert_eq!(
                solver.entail(&Constraint::TyRelate(TyRelate::new(variable.clone(), ty))).await,
                expected
            );
            for rejected in [Ty::new_unit(&engine), Ty::new_star_error(&engine)] {
                assert_eq!(
                    solver
                        .entail(&Constraint::TyRelate(TyRelate::new(variable.clone(), rejected)))
                        .await,
                    Err(Error::Conflicted)
                );
            }
        }
    }

    async fn solve(
        solver: &mut Solver,
        constraint: Constraint,
        engine: &TrackedEngine,
    ) -> Result<Subst, Error> {
        let mut pending = vec![constraint];
        let mut subst = Subst::new_empty();

        while let Some(constraint) = pending.pop() {
            let constraint = constraint.apply_subst_or_clone(&subst, engine);
            match solver.entail(&constraint).await? {
                Step::Subst(new_subst) => subst.compose(&new_subst, engine),
                Step::Derived(constraints) => {
                    pending.extend(constraints.into_iter().map(|x| x.constraint));
                }
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

        let step = solver.entail(&Constraint::TyRelate(TyRelate::new(lesser, greater))).await;

        assert_eq!(step, Ok(Step::Derived(Vec::new())));
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

        let step = solver.entail(&Constraint::TyRelate(TyRelate::new(lesser, greater))).await;

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

        let step = solver.entail(&Constraint::TyRelate(TyRelate::new(lesser, greater))).await;

        let e3 = engine.intern(Ty::Inference(Inference::new(TyKind::EffectRow, 2)));
        let state_remainder = Ty::new_effect_row([state], Some(e3.clone()), &engine);
        let io_remainder = Ty::new_effect_row([io], Some(e3), &engine);
        assert_eq!(
            step,
            Ok(Step::Derived(vec![
                DerivedConstraint::new_type_application_matching(e1, state_remainder),
                DerivedConstraint::new_type_application_matching(io_remainder, e2),
            ]))
        );
    }

    // input: {IO | e1} = {Exn | e2}
    // premise: e1 and e2 are distinct open effect-row variables
    // output: e1 := {Exn | e3}, e2 := {IO | e3}
    #[tokio::test]
    async fn distinct_open_effect_rows_have_a_principal_shared_tail_substitution() {
        let engine = rayc_qbice::create_minimal_engine().await;
        let io = effect_label(1, &engine);
        let exn = effect_label(2, &engine);
        let mut solver = Solver::new(engine.clone());
        let e1 = solver.new_inference(TyKind::EffectRow);
        let e2 = solver.new_inference(TyKind::EffectRow);
        let e1_ty = engine.intern(Ty::Inference(e1));
        let e2_ty = engine.intern(Ty::Inference(e2));
        let lesser = Ty::new_effect_row([io.clone()], Some(e1_ty), &engine);
        let greater = Ty::new_effect_row([exn.clone()], Some(e2_ty), &engine);

        let subst =
            solve(&mut solver, Constraint::TyRelate(TyRelate::new(lesser, greater)), &engine)
                .await
                .expect("distinct open rows should unify through a common tail");

        let e3 = engine.intern(Ty::Inference(Inference::new(TyKind::EffectRow, 2)));
        assert_eq!(subst.get(&e1), Some(&Ty::new_effect_row([exn], Some(e3.clone()), &engine)));
        assert_eq!(subst.get(&e2), Some(&Ty::new_effect_row([io], Some(e3), &engine)));
    }

    // input: {IO | e} = {IO}
    // premise: e is an open effect-row variable
    // output: e := {}
    #[tokio::test]
    async fn open_effect_row_tail_closes_when_no_labels_remain() {
        let engine = rayc_qbice::create_minimal_engine().await;
        let io = effect_label(1, &engine);
        let mut solver = Solver::new(engine.clone());
        let e = solver.new_inference(TyKind::EffectRow);
        let e_ty = engine.intern(Ty::Inference(e));
        let open = Ty::new_effect_row([io.clone()], Some(e_ty), &engine);
        let closed = Ty::new_effect_row([io], None, &engine);

        let subst = solve(&mut solver, Constraint::TyRelate(TyRelate::new(open, closed)), &engine)
            .await
            .expect("the open tail should close");

        assert_eq!(subst.get(&e), Some(&Ty::new_effect_row([], None, &engine)));
    }

    // input: {IO | e} = {IO, IO}
    // premise: duplicate effect labels are significant
    // output: e := {IO}
    #[tokio::test]
    async fn duplicate_effect_label_remains_in_open_tail_solution() {
        let engine = rayc_qbice::create_minimal_engine().await;
        let io = effect_label(1, &engine);
        let mut solver = Solver::new(engine.clone());
        let e = solver.new_inference(TyKind::EffectRow);
        let e_ty = engine.intern(Ty::Inference(e));
        let open = Ty::new_effect_row([io.clone()], Some(e_ty), &engine);
        let duplicate = Ty::new_effect_row([io.clone(), io.clone()], None, &engine);

        let subst =
            solve(&mut solver, Constraint::TyRelate(TyRelate::new(open, duplicate)), &engine)
                .await
                .expect("the duplicate label should remain in the tail");

        assert_eq!(subst.get(&e), Some(&Ty::new_effect_row([io], None, &engine)));
    }

    // input: e = {IO | e}
    // premise: e is an effect-row inference variable
    // output: OccursCheckFailed
    #[tokio::test]
    async fn effect_row_inference_cannot_bind_to_a_row_containing_itself() {
        let engine = rayc_qbice::create_minimal_engine().await;
        let io = effect_label(1, &engine);
        let mut solver = Solver::new(engine.clone());
        let e = solver.new_inference(TyKind::EffectRow);
        let e_ty = engine.intern(Ty::Inference(e));
        let recursive_row = Ty::new_effect_row([io], Some(e_ty.clone()), &engine);

        let result =
            solve(&mut solver, Constraint::TyRelate(TyRelate::new(e_ty, recursive_row)), &engine)
                .await;

        assert_eq!(result, Err(Error::OccursCheckFailed));
    }

    // input: e = p
    // premise: e is unconstrained; p is a rigid effect-row variable
    // output: e := p, with no substitution for p
    #[tokio::test]
    async fn effect_inference_binds_to_rigid_effect_poly_var_without_rebinding_it() {
        let (engine, poly) = engine_with_effect_poly_var().await;
        let mut solver = Solver::new(engine.clone());
        let e = solver.new_inference(TyKind::EffectRow);
        let e_ty = engine.intern(Ty::Inference(e));
        let poly_ty = Ty::new_poly_var(poly, &engine);

        let subst =
            solve(&mut solver, Constraint::TyRelate(TyRelate::new(poly_ty.clone(), e_ty)), &engine)
                .await
                .expect("an unconstrained inference should bind to a rigid variable");

        assert_eq!(subst.get(&e), Some(&poly_ty));
        assert_eq!(subst.get(&poly), None);
    }

    // input: {IO | p} = {IO | e}
    // premise: p is rigid; e is an effect-row inference variable
    // output: e := p
    #[tokio::test]
    async fn matching_open_effect_rows_unify_their_tails_directly() {
        let (engine, poly) = engine_with_effect_poly_var().await;
        let io = effect_label(1, &engine);
        let mut solver = Solver::new(engine.clone());
        let inference = solver.new_inference(TyKind::EffectRow);
        let inference_ty = engine.intern(Ty::Inference(inference));
        let poly_ty = Ty::new_poly_var(poly, &engine);
        let rigid_row = Ty::new_effect_row([io.clone()], Some(poly_ty.clone()), &engine);
        let inferred_row = Ty::new_effect_row([io], Some(inference_ty), &engine);

        let subst = solve(
            &mut solver,
            Constraint::TyRelate(TyRelate::new(rigid_row, inferred_row)),
            &engine,
        )
        .await
        .expect("matching open rows should unify their tails");

        assert_eq!(subst.get(&inference), Some(&poly_ty));
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

        let step =
            solver.entail(&Constraint::TyRelate(TyRelate::new(inference_ty, row.clone()))).await;

        assert_eq!(step, Ok(Step::Subst(Subst::new_singleton(inference, row))));
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
            solve(&mut solver, Constraint::TyRelate(TyRelate::new(lesser, greater)), &engine).await;

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

        let subst =
            solve(&mut solver, Constraint::TyRelate(TyRelate::new(lesser, greater)), &engine)
                .await
                .expect("same-constructor occurrences should match positionally");

        assert_eq!(subst.get(&a), Some(&int32));
        assert_eq!(subst.get(&b), Some(&bool));
    }
}
