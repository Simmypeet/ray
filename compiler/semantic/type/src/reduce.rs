use qbice::storage::intern::Interned;
use rayc_qbice::TrackedEngine;

use crate::{
    constraint::outlives::OutlivesConstraints,
    fold::{TyFoldable, TyFolder},
    ty::Ty,
    where_clause::PredicateKind,
};

/// Rewrites the types in a value that the given equalities reduce.
///
/// The smart constructors already take every structural step, so only a
/// given can rewrite a nested type; rebuilding then takes the structural
/// steps the rewrite enabled.
pub trait Reduce: TyFoldable + Sync {
    /// Rewrites, in one pass, every nested type that matches a given, with the
    /// outlives constraints of the matches, or returns `None` if none does.
    #[allow(async_fn_in_trait)]
    async fn reduce(
        &self,
        engine: &TrackedEngine,
        givens: &[PredicateKind],
    ) -> Option<(Self, OutlivesConstraints)> {
        let mut folder = GivensFolder { givens, outlives: OutlivesConstraints::new() };
        let reduced = self.fold_with(&mut folder, engine).await?;
        Some((reduced, folder.outlives))
    }
}

impl<T: TyFoldable + Sync> Reduce for T {}

/// The [`TyFolder`] behind [`Reduce`], which rewrites a type by the first
/// given equality whose left side it matches.
struct GivensFolder<'a> {
    givens: &'a [PredicateKind],
    outlives: OutlivesConstraints,
}

impl TyFolder for GivensFolder<'_> {
    async fn fold_ty(&mut self, ty: &Interned<Ty>, engine: &TrackedEngine) -> Option<Interned<Ty>> {
        // A type is matched once its nested types are rewritten, since that
        // may make it the left side of a given. The replacement is not folded
        // again in the same pass.
        let folded = Ty::super_fold_with(ty, self, engine).await;
        let current = folded.as_ref().unwrap_or(ty);

        for equality in self.givens.iter().filter_map(PredicateKind::as_equality) {
            // Lifetimes never decide whether a given applies, just as they
            // never decide which instance is selected. Projection arguments
            // are invariant, so the lifetimes of a matching given are related
            // by equality.
            if equality.right() == current {
                continue;
            }
            if let Some(outlives) =
                Ty::equal_modulo_lifetimes(equality.left(), current, engine).await
            {
                self.outlives = std::mem::take(&mut self.outlives).union(outlives);
                return Some(equality.right().clone());
            }
        }

        folded
    }
}

#[cfg(test)]
mod tests {
    use qbice::storage::intern::Interned;
    use rayc_qbice::TrackedEngine;
    use rayc_symbol::SymbolID;
    use rayc_target::TargetID;

    use super::Reduce;
    use crate::ty::{
        Integer, Primitive, Ty, TyKind, args::Args, effect_row::EffectLabel, inference::Inference,
    };

    fn effect_label(id: u128, engine: &TrackedEngine) -> Interned<EffectLabel> {
        let symbol_id = TargetID::TEST.make_global(SymbolID::from_u128(id));
        engine.intern(EffectLabel::new(symbol_id, Args::new([], engine)))
    }

    /// Creates an engine that knows `member_id` as an associated type of
    /// kind `kind`.
    async fn engine_with_associated_type(
        member_id: rayc_symbol::GlobalSymbolID,
        kind: TyKind,
    ) -> TrackedEngine {
        use std::{collections::HashMap, sync::Arc};

        use rayc_qbice::{Engine, InMemoryFactory, PrecomputedExecutor};

        let mut engine = Engine::new_with(
            qbice::serialize::Plugin::default(),
            InMemoryFactory,
            qbice::stable_hash::SeededStableHasherBuilder::new(0),
        )
        .await
        .unwrap();
        engine.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
            crate::associated_type_kind::Key { symbol_id: member_id },
            kind,
        )]))));
        Arc::new(engine).tracked().await
    }

    // input: {| int32}
    // premise: an effect row with no labels is represented only by its tail
    // output: int32, already at construction
    #[tokio::test]
    async fn effect_row_of_only_a_tail_is_its_tail() {
        let engine = rayc_qbice::create_minimal_engine().await;
        let int_ty = Ty::new_primitive(Primitive::Integer(Integer::Int32), &engine);

        assert_eq!(Ty::new_effect_row([], Some(int_ty.clone()), &engine), int_ty);
    }

    // input: {IO | {State | e}}
    // premise: the tail is a concrete effect row
    // output: {IO, State | e}, already at construction
    #[tokio::test]
    async fn nested_effect_rows_are_flattened() {
        let engine = rayc_qbice::create_minimal_engine().await;
        let io = effect_label(1, &engine);
        let state = effect_label(2, &engine);
        let tail = engine.intern(Ty::Inference(Inference::new(TyKind::EffectRow, 0)));
        let inner = Ty::new_effect_row([state.clone()], Some(tail.clone()), &engine);
        let outer = Ty::new_effect_row([io.clone()], Some(inner), &engine);

        let Ty::EffectRow(row) = &*outer else { panic!("expected an effect row") };
        assert_eq!(row.labels().cloned().collect::<Vec<_>>(), [io, state]);
        assert_eq!(row.tail(), Some(&tail));
    }

    // input: {IO | {}}
    // premise: the tail is an empty effect row
    // output: {IO}, already at construction
    #[tokio::test]
    async fn empty_effect_row_tail_is_removed() {
        let engine = rayc_qbice::create_minimal_engine().await;
        let io = effect_label(1, &engine);
        let empty = Ty::new_effect_row([], None, &engine);
        let row = Ty::new_effect_row([io.clone()], Some(empty), &engine);

        let Ty::EffectRow(row) = &*row else { panic!("expected an effect row") };
        assert_eq!(row.labels().cloned().collect::<Vec<_>>(), [io]);
        assert_eq!(row.tail(), None);
    }

    // input: (c.Item, c.Item)
    // premise: c.Item = int32 is given
    // output: (int32, int32), since one pass rewrites every match
    #[tokio::test]
    async fn reduction_rewrites_every_match_in_one_pass() {
        use crate::{
            ty::self_instance::SelfInstance,
            where_clause::{AssociatedTypeEquality, PredicateKind},
        };

        let trait_id = TargetID::TEST.make_global(SymbolID::from_u128(1));
        let member_id = TargetID::TEST.make_global(SymbolID::from_u128(2));
        let engine = engine_with_associated_type(member_id, TyKind::Star).await;
        let dictionary = engine.intern(Ty::SelfInstance(SelfInstance::new(trait_id)));
        let projection = Ty::new_instance_associated(member_id, dictionary, [], &engine).await;
        let int_ty = Ty::new_primitive(Primitive::Integer(Integer::Int32), &engine);
        let givens = [PredicateKind::AssociatedTypeEquality(AssociatedTypeEquality::new(
            projection.clone(),
            int_ty.clone(),
        ))];
        let tuple =
            Ty::new_tuple(engine.intern_unsized([projection.clone(), projection.clone()]), &engine);
        let expected = Ty::new_tuple(engine.intern_unsized([int_ty.clone(), int_ty]), &engine);

        assert_eq!(
            tuple.reduce(&engine, &givens).await.map(|(reduced, _)| reduced),
            Some(expected)
        );
    }

    // input: {IO}
    // premise: the row has no type that a given rewrites
    // output: no reduction
    #[tokio::test]
    async fn irreducible_effect_row_returns_none() {
        let engine = rayc_qbice::create_minimal_engine().await;
        let row = Ty::new_effect_row([effect_label(1, &engine)], None, &engine);

        assert_eq!(row.reduce(&engine, &[]).await.map(|(reduced, _)| reduced), None);
    }

    // input: c.Item and the enclosing tuple (c.Item,)
    // premise: c.Item = int32 is given
    // output: c.Item reduces to int32; the tuple reduces to (int32,)
    #[tokio::test]
    async fn reduction_uses_givens_in_descendants() {
        use crate::{
            ty::self_instance::SelfInstance,
            where_clause::{AssociatedTypeEquality, PredicateKind},
        };

        let trait_id = TargetID::TEST.make_global(SymbolID::from_u128(1));
        let member_id = TargetID::TEST.make_global(SymbolID::from_u128(2));
        let engine = engine_with_associated_type(member_id, TyKind::Star).await;
        let dictionary = engine.intern(Ty::SelfInstance(SelfInstance::new(trait_id)));
        let projection = Ty::new_instance_associated(member_id, dictionary, [], &engine).await;
        let int_ty = Ty::new_primitive(Primitive::Integer(Integer::Int32), &engine);
        let givens = [PredicateKind::AssociatedTypeEquality(AssociatedTypeEquality::new(
            projection.clone(),
            int_ty.clone(),
        ))];

        assert_eq!(
            projection.reduce(&engine, &givens).await.map(|(reduced, _)| reduced),
            Some(int_ty.clone())
        );
        let tuple = Ty::new_tuple(engine.intern_unsized([projection]), &engine);
        assert_eq!(
            tuple.reduce(&engine, &givens).await.map(|(reduced, _)| reduced),
            Some(Ty::new_tuple(engine.intern_unsized([int_ty]), &engine))
        );
    }

    // input: {IO | c.Effect}
    // premise: c.Effect = {State} is given
    // output: {IO, State}, since rebuilding the row flattens its new tail
    #[tokio::test]
    async fn rewriting_a_row_tail_flattens_the_row() {
        use crate::{
            ty::self_instance::SelfInstance,
            where_clause::{AssociatedTypeEquality, PredicateKind},
        };

        let trait_id = TargetID::TEST.make_global(SymbolID::from_u128(1));
        let member_id = TargetID::TEST.make_global(SymbolID::from_u128(2));
        let engine = engine_with_associated_type(member_id, TyKind::EffectRow).await;
        let io = effect_label(3, &engine);
        let state = effect_label(4, &engine);
        let dictionary = engine.intern(Ty::SelfInstance(SelfInstance::new(trait_id)));
        let projection = Ty::new_instance_associated(member_id, dictionary, [], &engine).await;
        let givens = [PredicateKind::AssociatedTypeEquality(AssociatedTypeEquality::new(
            projection.clone(),
            Ty::new_effect_row([state.clone()], None, &engine),
        ))];
        let row = Ty::new_effect_row([io.clone()], Some(projection), &engine);

        let (reduced, _) = row.reduce(&engine, &givens).await.expect("the tail is given");
        let Ty::EffectRow(reduced) = &*reduced else { panic!("expected an effect row") };
        assert_eq!(reduced.labels().cloned().collect::<Vec<_>>(), [io, state]);
        assert_eq!(reduced.tail(), None);
    }

    // input: this.Out['?1]
    // premise: this.Out['?0] = &'?0 int32 is given
    // output: &'?0 int32, with '?0: '?1 and '?1: '?0
    #[tokio::test]
    async fn given_equalities_match_modulo_lifetimes() {
        use crate::{
            ty::{Mutability, lifetime::Lifetime, self_instance::SelfInstance},
            where_clause::{AssociatedTypeEquality, OutlivesPredicate, PredicateKind},
        };

        let trait_id = TargetID::TEST.make_global(SymbolID::from_u128(1));
        let member_id = TargetID::TEST.make_global(SymbolID::from_u128(2));
        let engine = engine_with_associated_type(member_id, TyKind::Star).await;
        let this = engine.intern(Ty::SelfInstance(SelfInstance::new(trait_id)));
        let region =
            |index| Ty::new_lifetime(Lifetime::Region(rayc_arena::ID::new(index)), &engine);
        let (given, used) = (region(0), region(1));
        let out = async |lifetime: &Interned<Ty>| {
            Ty::new_instance_associated(member_id, this.clone(), [lifetime.clone()], &engine).await
        };
        let int_ty = Ty::new_primitive(Primitive::Integer(Integer::Int32), &engine);
        let reference = Ty::new_reference(given.clone(), int_ty, Mutability::Immutable, &engine);
        let givens = [PredicateKind::AssociatedTypeEquality(AssociatedTypeEquality::new(
            out(&given).await,
            reference.clone(),
        ))];
        let reduced = out(&used).await.reduce(&engine, &givens).await;

        let outlives = [
            OutlivesPredicate::new(given.clone(), used.clone()),
            OutlivesPredicate::new(used, given),
        ];
        assert_eq!(reduced, Some((reference, outlives.into_iter().collect())));
    }
}
