use qbice::{Identifiable, StableHash, storage::intern::Interned};
use rayc_qbice::TrackedEngine;

/// Performs a single reduction step.
pub trait Reduce {
    /// Returns the value after one reduction step, or [`None`] when neither the
    /// value nor any of its descendants can be reduced.
    #[must_use]
    fn reduce(&self, engine: &TrackedEngine) -> Option<Self>
    where
        Self: Sized;

    /// Reduces this value and all descendants to normal form.
    ///
    /// Reduction implementations must make strict progress toward termination.
    /// Current type reductions remove effect-row wrappers from a finite tree;
    /// future alias reductions must handle alias cycles before returning a
    /// step.
    #[must_use]
    fn normalize(&self, engine: &TrackedEngine) -> Self
    where
        Self: Sized + Clone + PartialEq,
    {
        let mut normalized = self.clone();
        while let Some(reduced) = normalized.reduce(engine) {
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
    fn reduce(&self, engine: &TrackedEngine) -> Option<Self> {
        self.iter().enumerate().find_map(|(index, value)| {
            value.reduce(engine).map(|reduced| {
                let mut values = self.to_vec();
                values[index] = reduced;
                engine.intern_unsized(values)
            })
        })
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

        assert_eq!(row.reduce(&engine), Some(int_ty));
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

        assert_eq!(outer.reduce(&engine), Some(expected));
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

        assert_eq!(row.reduce(&engine), Some(expected));
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

        assert_eq!(tuple.reduce(&engine), Some(expected));
    }

    // input: {IO}
    // premise: the row has neither a tail nor a reducible descendant
    // output: no reduction
    #[tokio::test]
    async fn irreducible_effect_row_returns_none() {
        let engine = rayc_qbice::create_minimal_engine().await;
        let row = Ty::new_effect_row([effect_label(1, &engine)], None, &engine);

        assert_eq!(row.reduce(&engine), None);
    }
}
