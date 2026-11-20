use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;

use crate::{
    fold::{TyFoldable, TyFolder},
    poly_var::{GlobalPolyVarID, get_poly_var_map},
    subst::{Subst, Substitutable},
    ty::{
        Ty,
        args::Args,
        inference::{GenInfer, Inference},
    },
    variance::{Variance, VarianceMap, get_variance},
};

mod effect_row2;

pub use effect_row2::{AtomState, EffectAtom, EffectRow2, Entry, GuardedRowEntry, Matching};

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct EffectLabel {
    effect_symbol_id: GlobalSymbolID,
    args: Args,
}

impl TyFoldable for Interned<EffectLabel> {
    async fn fold_with(&self, folder: &mut impl TyFolder, engine: &TrackedEngine) -> Option<Self> {
        self.args.fold_with(folder, engine).await.map(|args| {
            engine.intern(EffectLabel { effect_symbol_id: self.effect_symbol_id, args })
        })
    }
}

impl TyFoldable for EffectLabel {
    async fn fold_with(&self, folder: &mut impl TyFolder, engine: &TrackedEngine) -> Option<Self> {
        self.args
            .fold_with(folder, engine)
            .await
            .map(|args| EffectLabel { effect_symbol_id: self.effect_symbol_id, args })
    }
}

impl EffectLabel {
    #[must_use]
    pub const fn new(effect_symbol_id: GlobalSymbolID, args: Args) -> Self {
        Self { effect_symbol_id, args }
    }

    /// Returns the label of the effect `effect_symbol_id` instantiated with
    /// `substitution`.
    pub async fn instantiated(
        effect_symbol_id: GlobalSymbolID,
        substitution: &Subst,
        engine: &TrackedEngine,
    ) -> Interned<Self> {
        // The effect applied to its own parameters.
        let parameters = engine.get_poly_var_map(effect_symbol_id).await;
        let arguments = Args::new(
            parameters.iter().map(|(parameter_id, _)| {
                Ty::new_poly_var(GlobalPolyVarID::new(effect_symbol_id, parameter_id), engine)
            }),
            engine,
        );
        let label = engine.intern(Self::new(effect_symbol_id, arguments));

        label.apply_subst_or_clone(substitution, engine).await
    }

    #[must_use]
    pub const fn effect_symbol_id(&self) -> GlobalSymbolID { self.effect_symbol_id }

    #[must_use]
    pub const fn arguments(&self) -> &Args { &self.args }

    #[must_use]
    pub fn has_arguments(&self) -> bool { !self.args.is_empty() }

    /// Returns each argument with the variance of its position.
    pub fn arguments_with_variance<'a>(
        &'a self,
        effect_variances: &'a VarianceMap,
    ) -> impl Iterator<Item = (&'a Interned<Ty>, Variance)> {
        self.args
            .interned_iter()
            .enumerate()
            .map(move |(index, arg)| (arg, effect_variances.get_by_index(index)))
    }

    /// Returns each argument's variance within an `ambient` position, querying
    /// the effect's variances only when they can matter.
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
    /// Creates the row as given; [`Ty::new_effect_row`] is the constructor
    /// that keeps rows flattened.
    #[must_use]
    pub(super) fn new(
        labels: impl IntoIterator<Item = Interned<EffectLabel>>,
        tail: Option<Interned<Ty>>,
        engine: &TrackedEngine,
    ) -> Self {
        Self { labels: engine.intern_unsized(labels.into_iter().collect::<Vec<_>>()), tail }
    }

    #[must_use]
    pub fn labels(&self) -> impl ExactSizeIterator<Item = &Interned<EffectLabel>> {
        self.labels.iter()
    }

    #[must_use]
    pub const fn tail(&self) -> Option<&Interned<Ty>> { self.tail.as_ref() }

    /// Returns whether this row is syntactically empty; a row variable tail is
    /// not, since it may be instantiated with an effect.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.labels.is_empty() && self.tail.as_ref().is_none_or(|tail| tail.is_empty_effect_row())
    }

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

    /// Matches the labels of this row with those of `other` as Koka-style
    /// scoped labels, where only labels of different effects commute.
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

impl EffectRow {
    /// Folds the labels before the tail, flattening a tail that became a row;
    /// see [`Ty::super_fold_with`].
    pub(crate) async fn fold_with(
        &self,
        folder: &mut impl TyFolder,
        engine: &TrackedEngine,
    ) -> Option<Interned<Ty>> {
        let new_labels = self.labels.fold_with(folder, engine).await;
        let new_tail = match &self.tail {
            Some(tail) => tail.fold_with(folder, engine).await,
            None => None,
        };

        self.rebuild(new_labels, new_tail, engine)
    }

    /// Rebuilds the row with whichever parts changed, or returns `None` when
    /// neither did.
    fn rebuild(
        &self,
        new_labels: Option<Interned<[Interned<EffectLabel>]>>,
        new_tail: Option<Interned<Ty>>,
        engine: &TrackedEngine,
    ) -> Option<Interned<Ty>> {
        if new_labels.is_none() && new_tail.is_none() {
            return None;
        }

        let labels = new_labels.unwrap_or_else(|| self.labels.clone());
        let tail = new_tail.or_else(|| self.tail.clone());
        Some(Ty::new_effect_row(labels.iter().cloned(), tail, engine))
    }

    /// Returns the row with a fresh row variable as its tail if it is closed.
    pub(super) fn open_closed_row(
        &self,
        infer_gen: &mut impl GenInfer,
        engine: &TrackedEngine,
    ) -> Option<Interned<Ty>> {
        // A flattened row's tail is never a row, so an open row stays open.
        if self.tail.is_some() {
            return None;
        }

        let new_tail = engine.intern(Ty::Inference(infer_gen.gen_effect_row_infer()));
        Some(Ty::new_effect_row(self.labels.iter().cloned(), Some(new_tail), engine))
    }
}
