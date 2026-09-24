use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;

use crate::{
    reduce::Reduce,
    rewrite::{Rewrite, TyRewriter},
    subst::Substitutable,
    ty::{
        Ty,
        args::Args,
        inference::{GenInfer, Inference},
    },
};

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct EffectLabel {
    effect_symbol_id: GlobalSymbolID,
    args: Args,
}

impl Reduce for Interned<EffectLabel> {
    async fn reduce(
        &self,
        engine: &rayc_qbice::TrackedEngine,
        givens: &[crate::where_clause::PredicateKind],
    ) -> Option<Self> {
        self.args.reduce(engine, givens).await.map(|args| {
            engine.intern(EffectLabel { effect_symbol_id: self.effect_symbol_id, args })
        })
    }
}

impl Substitutable for Interned<EffectLabel> {
    fn apply_subst(
        &self,
        subst: &crate::subst::Subst,
        engine: &rayc_qbice::TrackedEngine,
    ) -> Option<Self>
    where
        Self: Sized,
    {
        self.args.apply_subst(subst, engine).map(|args| {
            engine.intern(EffectLabel { effect_symbol_id: self.effect_symbol_id, args })
        })
    }
}

impl Rewrite for Interned<EffectLabel> {
    fn rewrite(&self, rewriter: &mut impl TyRewriter, engine: &TrackedEngine) -> Option<Self> {
        self.args.rewrite(rewriter, engine).map(|args| {
            engine.intern(EffectLabel { effect_symbol_id: self.effect_symbol_id, args })
        })
    }
}

impl EffectLabel {
    #[must_use]
    pub const fn new(effect_symbol_id: GlobalSymbolID, args: Args) -> Self {
        Self { effect_symbol_id, args }
    }

    #[must_use]
    pub const fn effect_symbol_id(&self) -> GlobalSymbolID { self.effect_symbol_id }

    #[must_use]
    pub const fn arguments(&self) -> &Args { &self.args }

    #[must_use]
    pub fn has_arguments(&self) -> bool { !self.args.is_empty() }

    #[must_use]
    pub fn structural_match<'a>(
        &'a self,
        other: &'a Self,
    ) -> Option<impl Iterator<Item = (&'a Interned<Ty>, &'a Interned<Ty>)>> {
        (self.effect_symbol_id == other.effect_symbol_id)
            .then(|| self.args.structural_match(&other.args))
            .flatten()
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct EffectRow {
    labels: Interned<[Interned<EffectLabel>]>,
    tail: Option<Interned<Ty>>,
}

impl EffectRow {
    #[must_use]
    pub fn new(
        labels: impl IntoIterator<Item = Interned<EffectLabel>>,
        tail: Option<Interned<Ty>>,
        engine: &rayc_qbice::TrackedEngine,
    ) -> Self {
        Self { labels: engine.intern_unsized(labels.into_iter().collect::<Vec<_>>()), tail }
    }

    #[must_use]
    pub fn labels(&self) -> impl ExactSizeIterator<Item = &Interned<EffectLabel>> {
        self.labels.iter()
    }

    #[must_use]
    pub const fn tail(&self) -> Option<&Interned<Ty>> { self.tail.as_ref() }

    pub fn interned_iter(&self) -> impl Iterator<Item = &Interned<Ty>> {
        self.labels.iter().flat_map(|x| x.args.interned_iter()).chain(self.tail.as_ref())
    }

    pub fn iter(&self) -> impl Iterator<Item = &Ty> {
        self.labels
            .iter()
            .flat_map(|x| x.args.iter())
            .chain(self.tail.as_ref().map(std::convert::AsRef::as_ref))
    }

    #[must_use]
    pub fn has_inference_variable(&self, ty: &Inference) -> bool {
        self.interned_iter().any(|x| x.has_inference_variable(ty))
    }
}

impl Reduce for EffectRow {
    async fn reduce(
        &self,
        engine: &rayc_qbice::TrackedEngine,
        givens: &[crate::where_clause::PredicateKind],
    ) -> Option<Self> {
        if let Some(Ty::EffectRow(tail_row)) = self.tail.as_deref() {
            // reduce the case like `{A, B | {}}` to `{A, B}`.
            if tail_row.labels.is_empty() && tail_row.tail.is_none() {
                return Some(Self { labels: self.labels.clone(), tail: None });
            }

            return Some(Self::new(
                self.labels.iter().cloned().chain(tail_row.labels.iter().cloned()),
                tail_row.tail.clone(),
                engine,
            ));
        }

        if let Some(labels) = self.labels.reduce(engine, givens).await {
            return Some(Self { labels, tail: self.tail.clone() });
        }

        if let Some(tail) = &self.tail
            && let Some(tail) = tail.reduce(engine, givens).await
        {
            return Some(Self { labels: self.labels.clone(), tail: Some(tail) });
        }
        None
    }
}

impl Substitutable for EffectRow {
    fn apply_subst(
        &self,
        subst: &crate::subst::Subst,
        engine: &rayc_qbice::TrackedEngine,
    ) -> Option<Self>
    where
        Self: Sized,
    {
        let new_labels = self.labels.apply_subst(subst, engine);
        let new_tail = self.tail.as_ref().and_then(|x| x.apply_subst(subst, engine));

        match (new_labels, new_tail) {
            (None, None) => None,
            (None, Some(new_tail)) => {
                Some(Self { labels: self.labels.clone(), tail: Some(new_tail) })
            }
            (Some(new_labels), None) => Some(Self { labels: new_labels, tail: self.tail.clone() }),
            (Some(new_labels), Some(new_tail)) => {
                Some(Self { labels: new_labels, tail: Some(new_tail) })
            }
        }
    }
}

impl Rewrite for EffectRow {
    fn rewrite(&self, rewriter: &mut impl TyRewriter, engine: &TrackedEngine) -> Option<Self> {
        // The labels are visited before the tail.
        let new_labels = self.labels.rewrite(rewriter, engine);
        let new_tail = self.tail.as_ref().and_then(|tail| tail.rewrite(rewriter, engine));

        if new_labels.is_none() && new_tail.is_none() {
            return None;
        }

        Some(Self {
            labels: new_labels.unwrap_or_else(|| self.labels.clone()),
            tail: new_tail.or_else(|| self.tail.clone()),
        })
    }
}

impl EffectRow {
    pub(super) fn open_closed_row(
        &self,
        infer_gen: &mut impl GenInfer,
        engine: &TrackedEngine,
    ) -> Option<Self> {
        if let Some(eng) = self.tail.as_ref() {
            let new_tail = Ty::open_closed_row(eng, infer_gen, engine);
            new_tail.map(|new_tail| Self { labels: self.labels.clone(), tail: Some(new_tail) })
        } else {
            let new_tail = engine.intern(Ty::Inference(infer_gen.gen_effect_row_infer()));
            Some(Self { labels: self.labels.clone(), tail: Some(new_tail) })
        }
    }
}
