use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;

use crate::{
    constraint::outlives::OutlivesConstraints,
    reduce::Reduce,
    rewrite::{Rewrite, RewriteAsync, TyRewriter, TyRewriterAsync},
    subst::Substitutable,
    ty::{
        Ty,
        args::Args,
        inference::{GenInfer, Inference},
    },
    variance::{Variance, VarianceMap, get_variance},
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
    ) -> Option<(Self, OutlivesConstraints)> {
        self.args.reduce(engine, givens).await.map(|(args, outlives)| {
            let label = EffectLabel { effect_symbol_id: self.effect_symbol_id, args };
            (engine.intern(label), outlives)
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

impl RewriteAsync for Interned<EffectLabel> {
    async fn rewrite_async(
        &self,
        rewriter: &mut impl TyRewriterAsync,
        engine: &TrackedEngine,
    ) -> Option<Self> {
        self.args.rewrite_async(rewriter, engine).await.map(|args| {
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

    /// Returns each argument with the variance of its position.
    ///
    /// `effect_variances` are the variances of the effect's parameters.
    ///
    /// # Panics
    ///
    /// If `effect_variances` has fewer variances than there are arguments.
    pub fn arguments_with_variance<'a>(
        &'a self,
        effect_variances: &'a VarianceMap,
    ) -> impl Iterator<Item = (&'a Interned<Ty>, Variance)> {
        self.args
            .interned_iter()
            .enumerate()
            .map(move |(index, arg)| (arg, effect_variances.get_by_index(index)))
    }

    /// Returns each argument with the variance of its position, when this
    /// label is in a position of variance `ambient`.
    ///
    /// An invariant or bivariant position absorbs every variance inside it,
    /// so the variances of the effect are only queried when `ambient` is
    /// covariant or contravariant.
    pub async fn arguments_with_ambient_variance(
        &self,
        ambient: Variance,
        engine: &TrackedEngine,
    ) -> impl Iterator<Item = (&Interned<Ty>, Variance)> {
        let effect_variances = match ambient {
            Variance::Covariant | Variance::Contravariant => {
                Some(engine.get_variance(self.effect_symbol_id).await)
            }
            Variance::Invariant | Variance::Bivariant => None,
        };

        self.args.interned_iter().enumerate().map(move |(index, arg)| {
            let variance = effect_variances
                .as_ref()
                .map_or(ambient, |variances| ambient.xform(variances.get_by_index(index)));
            (arg, variance)
        })
    }

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

    /// Matches the labels of this row with those of `other` as scoped labels,
    /// as in Koka: labels of different effects commute, while labels of the
    /// same effect keep their order. Each label of this row, in order, matches
    /// the first label of `other` of the same effect that no earlier label has
    /// matched, so `{State[a], State[b]}` matches `{State[x], State[y]}` with
    /// `a` against `x` and `b` against `y`.
    #[must_use]
    pub fn match_labels<'a>(&'a self, other: &'a Self) -> MatchedLabels<'a> {
        let mut matched = Vec::new();
        let mut unmatched_left = Vec::new();
        let mut unmatched_right = other.labels().collect::<Vec<_>>();

        for label in self.labels() {
            let position = unmatched_right
                .iter()
                .position(|candidate| candidate.effect_symbol_id() == label.effect_symbol_id());

            match position {
                Some(position) => matched.push((label, unmatched_right.remove(position))),
                None => unmatched_left.push(label),
            }
        }

        MatchedLabels { matched, unmatched_left, unmatched_right }
    }
}

/// The labels of two effect rows matched as scoped labels; see
/// [`EffectRow::match_labels`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchedLabels<'a> {
    matched: Vec<(&'a Interned<EffectLabel>, &'a Interned<EffectLabel>)>,
    unmatched_left: Vec<&'a Interned<EffectLabel>>,
    unmatched_right: Vec<&'a Interned<EffectLabel>>,
}

impl<'a> MatchedLabels<'a> {
    /// Returns each label of the left row with the label of the right row it
    /// matches, in the left row's order.
    #[must_use]
    pub fn matched(
        &self,
    ) -> impl ExactSizeIterator<Item = (&'a Interned<EffectLabel>, &'a Interned<EffectLabel>)> + '_
    {
        self.matched.iter().copied()
    }

    /// Returns the labels of the left row that match no label of the right.
    #[must_use]
    pub fn unmatched_left(&self) -> impl ExactSizeIterator<Item = &'a Interned<EffectLabel>> + '_ {
        self.unmatched_left.iter().copied()
    }

    /// Returns the labels of the right row that match no label of the left.
    #[must_use]
    pub fn unmatched_right(&self) -> impl ExactSizeIterator<Item = &'a Interned<EffectLabel>> + '_ {
        self.unmatched_right.iter().copied()
    }

    /// Returns whether every label of either row matches one of the other.
    #[must_use]
    pub const fn is_exact(&self) -> bool {
        self.unmatched_left.is_empty() && self.unmatched_right.is_empty()
    }
}

impl Reduce for EffectRow {
    async fn reduce(
        &self,
        engine: &rayc_qbice::TrackedEngine,
        givens: &[crate::where_clause::PredicateKind],
    ) -> Option<(Self, OutlivesConstraints)> {
        if let Some(Ty::EffectRow(tail_row)) = self.tail.as_deref() {
            // reduce the case like `{A, B | {}}` to `{A, B}`.
            if tail_row.labels.is_empty() && tail_row.tail.is_none() {
                let row = Self { labels: self.labels.clone(), tail: None };
                return Some((row, OutlivesConstraints::new()));
            }

            let row = Self::new(
                self.labels.iter().cloned().chain(tail_row.labels.iter().cloned()),
                tail_row.tail.clone(),
                engine,
            );
            return Some((row, OutlivesConstraints::new()));
        }

        if let Some((labels, outlives)) = self.labels.reduce(engine, givens).await {
            return Some((Self { labels, tail: self.tail.clone() }, outlives));
        }

        if let Some(tail) = &self.tail
            && let Some((tail, outlives)) = tail.reduce(engine, givens).await
        {
            return Some((Self { labels: self.labels.clone(), tail: Some(tail) }, outlives));
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

impl RewriteAsync for EffectRow {
    async fn rewrite_async(
        &self,
        rewriter: &mut impl TyRewriterAsync,
        engine: &TrackedEngine,
    ) -> Option<Self> {
        // The labels are visited before the tail.
        let new_labels = self.labels.rewrite_async(rewriter, engine).await;
        let new_tail = match &self.tail {
            Some(tail) => tail.rewrite_async(rewriter, engine).await,
            None => None,
        };

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
