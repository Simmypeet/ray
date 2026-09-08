use qbice::{Identifiable, StableHash, storage::intern::Interned};
use rayc_qbice::TrackedEngine;

/// Performs a single reduction step.
pub trait Reduce: Sync {
    /// Returns the value after one reduction step, or `None` if irreducible.
    #[allow(async_fn_in_trait)]
    async fn reduce(&self, engine: &TrackedEngine) -> Option<Self>
    where
        Self: Sized;

    /// Reduces this value and its descendants until no further step is
    /// available. Reduction implementations must make progress toward
    /// termination.
    #[allow(async_fn_in_trait)]
    async fn normalize(&self, engine: &TrackedEngine) -> Self
    where
        Self: Sized + Clone + PartialEq + Send,
    {
        let mut normalized = self.clone();
        while let Some(reduced) = normalized.reduce(engine).await {
            assert!(reduced != normalized, "reduction must make progress");
            normalized = reduced;
        }
        normalized
    }
}

impl<T> Reduce for Interned<[T]>
where
    T: Reduce + Clone + StableHash + Identifiable + Send + Sync + 'static,
{
    async fn reduce(&self, engine: &TrackedEngine) -> Option<Self> {
        for (index, value) in self.iter().enumerate() {
            if let Some(reduced) = value.reduce(engine).await {
                let mut values = self.to_vec();
                values[index] = reduced;
                return Some(engine.intern_unsized(values));
            }
        }
        None
    }
}

/// Expands a projection only when its instance and member correspondence are
/// known.
pub(crate) async fn reduce_instance_associated(
    associated: crate::ty::application::InstanceAssociatedView<'_>,
    engine: &TrackedEngine,
) -> Option<Interned<crate::ty::Ty>> {
    use rayc_symbol::{
        member::get_member_by_name,
        name::get_name,
        symbol_kind::{SymbolKind, get_symbol_kind},
    };

    use crate::{
        instance_member::get_instance_member,
        poly_var::{GlobalPolyVarID, build_subst_from_args, get_poly_var_map},
        subst::Substitutable,
        ty::{Ty, application::View},
        type_definition::get_type_definition,
    };

    let Ty::Application(instance) = associated.instance().as_ref() else {
        return None;
    };
    let View::Instance(instance) = instance.view() else {
        return None;
    };

    // The projection names a trait member, while the definition belongs to its
    // same-named implementation in the concrete instance.
    let name = engine.get_name(associated.symbol_id()).await;
    let member_id = engine.get_member_by_name(instance.symbol_id(), &name).await?;
    if engine.get_symbol_kind(member_id).await != SymbolKind::InstanceType {
        return None;
    }
    let member = engine.get_instance_member(member_id).await;
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
        let implementation_var = member.poly_var_substitution().get(&trait_var)?;
        let Ty::PolyVar(implementation_var) = implementation_var.as_ref() else {
            return None;
        };
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
    use crate::ty::{
        Primitive, Ty, TyKind,
        args::Args,
        effect_row::{EffectLabel, EffectRow},
        inference::Inference,
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

        assert_eq!(row.reduce(&engine).await, Some(int_ty));
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

        assert_eq!(outer.reduce(&engine).await, Some(expected));
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

        assert_eq!(row.reduce(&engine).await, Some(expected));
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

        assert_eq!(tuple.reduce(&engine).await, Some(expected));
    }

    // input: {IO}
    // premise: the row has neither a tail nor a reducible descendant
    // output: no reduction
    #[tokio::test]
    async fn irreducible_effect_row_returns_none() {
        let engine = rayc_qbice::create_minimal_engine().await;
        let row = Ty::new_effect_row([effect_label(1, &engine)], None, &engine);

        assert_eq!(row.reduce(&engine).await, None);
    }
}

#[cfg(test)]
mod associated_type_tests;
