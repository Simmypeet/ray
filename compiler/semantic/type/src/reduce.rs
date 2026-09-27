use qbice::{Identifiable, StableHash, storage::intern::Interned};
use rayc_qbice::TrackedEngine;
use rayc_symbol::{
    core_item::{CoreItem, get_core_item},
    member::get_member_by_name,
    name::get_name,
    symbol_kind::{SymbolKind, get_symbol_kind},
};

use crate::{
    constraint::outlives::OutlivesSink,
    instance_member::get_instance_member,
    poly_var::{GlobalPolyVarID, build_subst_from_args, get_poly_var_map},
    subst::Substitutable,
    ty::{Ty, application::View},
    type_definition::get_type_definition,
};

/// Performs a single reduction step.
pub trait Reduce: Sync {
    /// Returns the value after one reduction step, or `None` if irreducible.
    /// Given equalities rewrite left operands that are equal to the value
    /// ignoring lifetimes to their right operands, in slice order, after
    /// ordinary reduction at each type. Descendants use the same givens.
    /// Equalities whose right operand is equal to the value ignoring
    /// lifetimes do not count as progress.
    ///
    /// A given equality that matches relates each pair of corresponding
    /// lifetimes invariantly, and the resulting outlives constraints go to
    /// `outlives`.
    #[allow(async_fn_in_trait)]
    async fn reduce(
        &self,
        engine: &TrackedEngine,
        givens: &[crate::where_clause::PredicateKind],
        outlives: &mut OutlivesSink,
    ) -> Option<Self>
    where
        Self: Sized;
}

impl<T> Reduce for Interned<[T]>
where
    T: Reduce + Clone + StableHash + Identifiable + Send + Sync + 'static,
{
    async fn reduce(
        &self,
        engine: &TrackedEngine,
        givens: &[crate::where_clause::PredicateKind],
        outlives: &mut OutlivesSink,
    ) -> Option<Self> {
        for (index, value) in self.iter().enumerate() {
            if let Some(reduced) = value.reduce(engine, givens, outlives).await {
                let mut values = self.to_vec();
                values[index] = reduced;
                return Some(engine.intern_unsized(values));
            }
        }
        None
    }
}

/// Expands a projection only when its instance and member correspondence are
/// known. This performs one definition expansion; further reduction uses the
/// ordinary [`Reduce`] traversal.
///
/// TODO: diagnose recursive associated types during normalization. A future
/// normalization context could track implementation member IDs on the active
/// expansion stack, catching direct and mutual recursion even when arguments
/// grow (for example, `Item[a] = Instance.Item[(a, a)]`). Expand supplied
/// arguments before entering the definition so finite nesting such as
/// `Identity.Item[Identity.Item[int32]]` remains valid. Preserve abstract
/// dictionary projections and report cycles through semantic diagnostics.
/// Until then, recursive definitions may remain irreducible or fail to
/// terminate during normalization.
pub(crate) async fn reduce_instance_associated(
    associated: crate::ty::application::InstanceAssociatedView<'_>,
    engine: &TrackedEngine,
) -> Option<Interned<crate::ty::Ty>> {
    // Closure dictionaries carry their signature directly, even during inference.
    if let Ty::Application(application) = &**associated.instance()
        && let View::DefInstance(closure) = application.view()
    {
        if !associated.args().is_empty() {
            return None;
        }
        let closure = closure.unwrap_as_closure_view();
        let member = associated.symbol_id();
        return if member == engine.get_core_item(CoreItem::DefArgs).await {
            Some(Ty::new_tuple(engine.intern_unsized(closure.params().to_vec()), engine))
        } else if member == engine.get_core_item(CoreItem::DefReturn).await {
            Some(closure.return_type().clone())
        } else if member == engine.get_core_item(CoreItem::DefEffect).await {
            Some(closure.effect_row().clone())
        } else {
            None
        };
    }

    let instance = associated.instance().as_instance_view()?;

    // The projection names a trait member, while the definition belongs to its
    // same-named implementation in the concrete instance.
    let name = engine.get_name(associated.symbol_id()).await;
    let member_id = engine.get_member_by_name(instance.symbol_id(), &name).await?;
    if engine.get_symbol_kind(member_id).await != SymbolKind::InstanceType {
        return None;
    }
    let member = engine.get_instance_member(member_id).await?;
    if member.trait_member_id() != associated.symbol_id() {
        return None;
    }

    let trait_poly_vars = engine.get_poly_var_map(associated.symbol_id()).await;
    if trait_poly_vars.len() != associated.args().len() {
        return None;
    }

    // Enclosing variables are supplied by the instance application. Translate
    // member arguments through the checked trait-to-implementation mapping.
    let mut substitution =
        engine.build_subst_from_args(instance.symbol_id(), instance.args()).await;

    for ((id, _), argument) in trait_poly_vars.iter().zip(associated.args()) {
        let trait_var = GlobalPolyVarID::new(associated.symbol_id(), id);
        let implementation_var = member.poly_var_substitution().get(&trait_var)?.as_poly_var()?;

        substitution.insert(*implementation_var, argument.clone());
    }

    let definition = engine.get_type_definition(member_id).await;
    Some(definition.apply_subst_or_clone(&substitution, engine))
}

#[cfg(test)]
mod tests {
    use qbice::storage::intern::Interned;
    use rayc_qbice::TrackedEngine;
    use rayc_symbol::SymbolID;
    use rayc_target::TargetID;

    use super::Reduce;
    use crate::{
        constraint::outlives::OutlivesSink,
        ty::{
            Primitive, Ty, TyKind,
            args::Args,
            effect_row::{EffectLabel, EffectRow},
            inference::Inference,
        },
    };

    fn effect_label(id: u128, engine: &TrackedEngine) -> Interned<EffectLabel> {
        let symbol_id = TargetID::TEST.make_global(SymbolID::from_u128(id));
        engine.intern(EffectLabel::new(symbol_id, Args::new([], engine)))
    }

    // input: {| int32}
    // premise: an effect row with no labels is represented only by its tail
    // output: int32
    #[tokio::test]
    async fn empty_effect_row_reduces_to_its_tail() {
        let engine = rayc_qbice::create_minimal_engine().await;
        let int_ty = Ty::new_primitive(Primitive::Int32, &engine);
        let row = Ty::new_effect_row([], Some(int_ty.clone()), &engine);

        assert_eq!(row.reduce(&engine, &[], &mut OutlivesSink::dropping()).await, Some(int_ty));
    }

    // input: {IO | {State | e}}
    // premise: the tail is a concrete effect row
    // output: {IO, State | e}
    #[tokio::test]
    async fn nested_effect_rows_reduce_by_concatenating_labels() {
        let engine = rayc_qbice::create_minimal_engine().await;
        let io = effect_label(1, &engine);
        let state = effect_label(2, &engine);
        let tail = engine.intern(Ty::Inference(Inference::new(TyKind::EffectRow, 0)));
        let inner = Ty::new_effect_row([state.clone()], Some(tail.clone()), &engine);
        let outer = EffectRow::new([io.clone()], Some(inner), &engine);
        let expected = EffectRow::new([io, state], Some(tail), &engine);

        assert_eq!(outer.reduce(&engine, &[], &mut OutlivesSink::dropping()).await, Some(expected));
    }

    // input: {IO | {}}
    // premise: the tail is an empty effect row
    // output: {IO}
    #[tokio::test]
    async fn empty_effect_row_tail_is_removed() {
        let engine = rayc_qbice::create_minimal_engine().await;
        let io = effect_label(1, &engine);
        let empty = Ty::new_effect_row([], None, &engine);
        let row = EffectRow::new([io.clone()], Some(empty), &engine);
        let expected = EffectRow::new([io], None, &engine);

        assert_eq!(row.reduce(&engine, &[], &mut OutlivesSink::dropping()).await, Some(expected));
    }

    // input: ({| int32}, {| bool})
    // premise: reduction is a single, left-to-right step
    // output: (int32, {| bool})
    #[tokio::test]
    async fn reduction_descends_to_only_the_first_reducible_child() {
        let engine = rayc_qbice::create_minimal_engine().await;
        let int_ty = Ty::new_primitive(Primitive::Int32, &engine);
        let bool_ty = Ty::new_primitive(Primitive::Bool, &engine);
        let first = Ty::new_effect_row([], Some(int_ty.clone()), &engine);
        let second = Ty::new_effect_row([], Some(bool_ty), &engine);
        let tuple = Ty::new_tuple(engine.intern_unsized([first, second.clone()]), &engine);
        let expected = Ty::new_tuple(engine.intern_unsized([int_ty, second]), &engine);

        assert_eq!(tuple.reduce(&engine, &[], &mut OutlivesSink::dropping()).await, Some(expected));
    }

    // input: {IO}
    // premise: the row has neither a tail nor a reducible descendant
    // output: no reduction
    #[tokio::test]
    async fn irreducible_effect_row_returns_none() {
        let engine = rayc_qbice::create_minimal_engine().await;
        let row = Ty::new_effect_row([effect_label(1, &engine)], None, &engine);

        assert_eq!(row.reduce(&engine, &[], &mut OutlivesSink::dropping()).await, None);
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

        let engine = rayc_qbice::create_minimal_engine().await;
        let trait_id = TargetID::TEST.make_global(SymbolID::from_u128(1));
        let member_id = TargetID::TEST.make_global(SymbolID::from_u128(2));
        let dictionary = engine.intern(Ty::SelfInstance(SelfInstance::new(trait_id)));
        let projection = Ty::new_instance_associated(member_id, dictionary, [], &engine);
        let int_ty = Ty::new_primitive(Primitive::Int32, &engine);
        let givens = [PredicateKind::AssociatedTypeEquality(AssociatedTypeEquality::new(
            projection.clone(),
            int_ty.clone(),
        ))];

        assert_eq!(
            projection.reduce(&engine, &givens, &mut OutlivesSink::dropping()).await,
            Some(int_ty.clone())
        );
        let tuple = Ty::new_tuple(engine.intern_unsized([projection]), &engine);
        assert_eq!(
            tuple.reduce(&engine, &givens, &mut OutlivesSink::dropping()).await,
            Some(Ty::new_tuple(engine.intern_unsized([int_ty]), &engine))
        );
    }

    // input: an empty effect row with a tail variable
    // premise: a given maps the row to a different row
    // output: ordinary reduction returns the tail before consulting givens
    #[tokio::test]
    async fn reduction_prefers_structural_steps() {
        use crate::{
            ty::{effect_row::EffectRow, inference::Inference},
            where_clause::{AssociatedTypeEquality, PredicateKind},
        };

        let engine = rayc_qbice::create_minimal_engine().await;
        let tail = engine.intern(Ty::Inference(Inference::new(TyKind::EffectRow, 0)));
        let row = engine.intern(Ty::EffectRow(EffectRow::new([], Some(tail.clone()), &engine)));
        let other = engine.intern(Ty::EffectRow(EffectRow::new([], None, &engine)));
        let givens = [PredicateKind::AssociatedTypeEquality(AssociatedTypeEquality::new(
            row.clone(),
            other,
        ))];

        assert_eq!(row.reduce(&engine, &givens, &mut OutlivesSink::dropping()).await, Some(tail));
    }

    // input: this.Out['?1]
    // premise: this.Out['?0] = &'?0 int32 is given
    // output: &'?0 int32, with '?0: '?1 and '?1: '?0
    #[tokio::test]
    async fn given_equalities_match_modulo_lifetimes() {
        use crate::{
            constraint::outlives::OutlivesConstraint,
            ty::{Mutability, lifetime::Lifetime, self_instance::SelfInstance},
            where_clause::{AssociatedTypeEquality, PredicateKind},
        };

        let engine = rayc_qbice::create_minimal_engine().await;
        let trait_id = TargetID::TEST.make_global(SymbolID::from_u128(1));
        let member_id = TargetID::TEST.make_global(SymbolID::from_u128(2));
        let this = engine.intern(Ty::SelfInstance(SelfInstance::new(trait_id)));
        let region =
            |index| Ty::new_lifetime(Lifetime::Region(rayc_arena::ID::new(index)), &engine);
        let (given, used) = (region(0), region(1));
        let out = |lifetime: &Interned<Ty>| {
            Ty::new_instance_associated(member_id, this.clone(), [lifetime.clone()], &engine)
        };
        let int_ty = Ty::new_primitive(Primitive::Int32, &engine);
        let reference = Ty::new_reference(given.clone(), int_ty, Mutability::Immutable, &engine);
        let givens = [PredicateKind::AssociatedTypeEquality(AssociatedTypeEquality::new(
            out(&given),
            reference.clone(),
        ))];
        let mut outlives = OutlivesSink::keeping();

        let reduced = out(&used).reduce(&engine, &givens, &mut outlives).await;

        assert_eq!(reduced, Some(reference));
        assert_eq!(outlives.into_constraints(), vec![
            OutlivesConstraint::new(given.clone(), used.clone()),
            OutlivesConstraint::new(used, given),
        ]);
    }
}

#[cfg(test)]
mod associated_type_tests;
