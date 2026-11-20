use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_hash::FxHashMap;
use rayc_qbice::TrackedEngine;

use crate::{
    fold::{TyFoldable, TyFolder, fold_pair},
    ty::{
        Ty, TyKind, application::View, effect_row::EffectLabel, inference::Inference,
        inspect::LifetimeErasable,
    },
};

/// An opaque effect row atom, which we can't look inside what's in it
/// currently.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct EffectAtom(Interned<Ty>);

impl EffectAtom {
    #[must_use]
    pub fn into_ty(self) -> Interned<Ty> { self.0 }
}

impl EffectAtom {
    #[must_use]
    pub fn get_atom_state(&self) -> AtomState {
        match self.0.as_ref() {
            Ty::Inference(_) => AtomState::Flexible,
            Ty::PolyVar(_) => AtomState::Rigid,
            Ty::Application(app) => match app.view() {
                View::InstanceAssociated(_) => {
                    if self.0.contains_non_lifetime_inference() {
                        AtomState::Stuck
                    } else {
                        AtomState::Rigid
                    }
                }
                _ => panic!(
                    "must be either polyvar, inference, or instance application, found {:?}",
                    self.0
                ),
            },
            _ => panic!(
                "must be either polyvar, inference, or instance application, found {:?}",
                self.0
            ),
        }
    }
}

/// Either a known concrete label or an opaque effect row atom, which can have
/// several labels or none at all.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum Entry {
    Label(EffectLabel),
    Atom(EffectAtom),
}

impl TyFoldable for Entry {
    async fn fold_with(&self, folder: &mut impl TyFolder, engine: &TrackedEngine) -> Option<Self> {
        match self {
            Self::Label(label) => label.fold_with(folder, engine).await.map(Self::Label),
            Self::Atom(atom) => {
                atom.0.fold_with(folder, engine).await.map(EffectAtom).map(Self::Atom)
            }
        }
    }
}

impl Entry {
    pub fn internred_iter(&self) -> impl Iterator<Item = &Interned<Ty>> {
        let (label, atom) = match self {
            Self::Label(label) => (Some(label), None),
            Self::Atom(atom) => (None, Some(atom)),
        };

        label
            .into_iter()
            .flat_map(|label| label.arguments().interned_iter())
            .chain(atom.into_iter().map(|x| &x.0))
    }

    pub fn iter(&self) -> impl Iterator<Item = &Ty> {
        let (label, atom) = match self {
            Self::Label(label) => (Some(label), None),
            Self::Atom(atom) => (None, Some(atom)),
        };

        label
            .into_iter()
            .flat_map(|label| label.arguments().iter())
            .chain(atom.into_iter().map(|x| &*x.0))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Matching<'x> {
    Label(&'x EffectLabel, &'x EffectLabel),
    Atom(&'x EffectAtom, &'x EffectAtom),
}

// An individual entry of an effect row with a set of "excluded" labels, which
// are labels that are not allowed to be in the row.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct GuardedRowEntry {
    source: Entry,
    /// The sorted and deduplicated set of "lifetime erased" labels
    excluded: Interned<[EffectLabel]>,
}

impl TyFoldable for GuardedRowEntry {
    async fn fold_with(&self, folder: &mut impl TyFolder, engine: &TrackedEngine) -> Option<Self> {
        let (source, excluded) = fold_pair(&self.source, &self.excluded, folder, engine).await?;
        Some(Self { source, excluded })
    }
}

impl GuardedRowEntry {
    pub async fn subsumes(&self, other: &Self, engine: &TrackedEngine) -> bool {
        match (&self.source, &other.source) {
            (Entry::Label(l), Entry::Label(r)) => {
                if l.effect_symbol_id != r.effect_symbol_id
                    || l.arguments().len() != r.arguments().len()
                {
                    return false;
                }

                for (l_arg, r_arg) in
                    l.arguments().interned_iter().zip(r.arguments().interned_iter())
                {
                    if Ty::corresponding_lifetimes(l_arg, r_arg, engine).await.is_none() {
                        return false;
                    }
                }
            }

            (Entry::Atom(l), Entry::Atom(r)) => {
                if Ty::corresponding_lifetimes(&l.0, &r.0, engine).await.is_none() {
                    return false;
                }
            }

            _ => return false,
        }

        self.excluded.len() <= other.excluded.len()
            // every excluded label in here must be in the other excluded labels
            && self.excluded.iter().all(|x| other.excluded.contains(x))
    }

    #[must_use]
    pub fn matchings<'x>(&'x self, other: &'x Self) -> Option<impl Iterator<Item = Matching<'x>>> {
        if self.excluded.len() != other.excluded.len()
            || self
                .excluded
                .iter()
                .zip(other.excluded.iter())
                .any(|(left, right)| left.effect_symbol_id != right.effect_symbol_id)
        {
            return None;
        }

        let (label_matching, atom_matching) = match (&self.source, &other.source) {
            (Entry::Label(left), Entry::Label(right)) => (Some(Matching::Label(left, right)), None),
            (Entry::Atom(left), Entry::Atom(right)) => (None, Some(Matching::Atom(left, right))),
            _ => return None,
        };

        let excluded_matching = self
            .excluded
            .iter()
            .zip(other.excluded.iter())
            .map(|(left, right)| Matching::Label(left, right));

        Some(excluded_matching.chain(label_matching).chain(atom_matching))
    }

    pub fn interned_iter(&self) -> impl Iterator<Item = &Interned<Ty>> {
        self.source
            .internred_iter()
            .chain(self.excluded.iter().flat_map(|x| x.arguments().interned_iter()))
    }

    pub fn iter(&self) -> impl Iterator<Item = &Ty> {
        self.source.iter().chain(self.excluded.iter().flat_map(|x| x.arguments().iter()))
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum LabelOrType {
    Label(EffectLabel),
    Type(Interned<Ty>),
}

impl LabelOrType {
    #[must_use]
    pub const fn as_label(&self) -> Option<&EffectLabel> {
        match self {
            Self::Label(label) => Some(label),
            Self::Type(_) => None,
        }
    }
}

/// A row of [`GuardedRowEntry`]s.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct EffectRow2 {
    /// Sorted and deduplicated by [`GuardedRowEntry::source`] with
    /// lifetime-erased.
    rows: Interned<[GuardedRowEntry]>,
}

impl EffectRow2 {
    /// A smart constructor.
    ///
    /// In the [`LabelOrType::Type`] case, the type must be either a polyvar,
    /// inference variable, instance associated type, or an [`EffectRow2`].
    pub async fn new_effect_row(
        entries: impl IntoIterator<Item = (LabelOrType, Vec<EffectLabel>)>,
        engine: &TrackedEngine,
    ) -> Interned<Ty> {
        let mut decideds = Self::flatten_and_decide(entries, engine).await;

        let mut erased_entry_keys = FxHashMap::default();

        for entry in &decideds {
            let erased_entry_key = entry.source.erase_lifetime(engine).await;
            erased_entry_keys.insert(entry.source.clone(), erased_entry_key);
        }

        // sort and dedup the entries by their source (with
        // lifetime-erased) to make sure the row is canonicalized.
        decideds.sort_unstable_by(|l, r| {
            let l_key = (erased_entry_keys.get(&l.source), &l.excluded);
            let r_key = (erased_entry_keys.get(&r.source), &r.excluded);
            l_key.cmp(&r_key)
        });
        decideds.dedup_by(|l, r| {
            let l_key = (erased_entry_keys.get(&l.source), &l.excluded);
            let r_key = (erased_entry_keys.get(&r.source), &r.excluded);
            l_key == r_key
        });

        // if one entry is subsumed by another, remove it (this is O(n^2), :P)
        let mut i = 0;
        while i < decideds.len() {
            let mut j = 0;
            while j < decideds.len() {
                if i != j && decideds[i].subsumes(&decideds[j], engine).await {
                    decideds.swap_remove(j);
                    if j < i {
                        i -= 1;
                    }
                } else {
                    j += 1;
                }
            }
            i += 1;
        }

        // sort one last time
        decideds.sort_unstable_by(|l, r| {
            let l_key = (erased_entry_keys.get(&l.source), &l.excluded);
            let r_key = (erased_entry_keys.get(&r.source), &r.excluded);
            l_key.cmp(&r_key)
        });

        // TODO: we probably can do a better job than this :-P
        match decideds.len() {
            0 => engine.intern(Ty::EffectRow2(Self { rows: engine.intern_unsized([]) })),

            // if it's a single EffectAtom with no excluded labels, we simply
            // unwrap it and return the atom directly
            1 => {
                if let Entry::Atom(atom) = &decideds[0].source
                    && decideds[0].excluded.is_empty()
                {
                    atom.0.clone()
                } else {
                    engine.intern(Ty::EffectRow2(Self { rows: engine.intern_unsized(decideds) }))
                }
            }

            _ => engine.intern(Ty::EffectRow2(Self { rows: engine.intern_unsized(decideds) })),
        }
    }

    async fn flatten_and_decide(
        entries: impl IntoIterator<Item = (LabelOrType, Vec<EffectLabel>)>,
        engine: &TrackedEngine,
    ) -> Vec<GuardedRowEntry> {
        let mut rows = Vec::new();

        for (entry, excludeds) in entries {
            match entry {
                LabelOrType::Type(interned) => match &*interned {
                    // flatten the effect row into individual entries
                    Ty::EffectRow2(effect_row2) => {
                        for row in effect_row2.rows.iter() {
                            let excludeds = excludeds
                                .iter()
                                .cloned()
                                .chain(row.excluded.iter().cloned())
                                .collect::<Vec<_>>();

                            if let Some(guarded_row_entry) =
                                Self::decide_entry(row.source.clone(), excludeds, engine).await
                            {
                                rows.push(guarded_row_entry);
                            }
                        }
                    }

                    Ty::Application(ap) if ap.is_instance_associated() => {
                        rows.push(
                            Self::decide_entry(
                                Entry::Atom(EffectAtom(interned)),
                                excludeds,
                                engine,
                            )
                            .await
                            .unwrap(),
                        );
                    }
                    Ty::PolyVar(_) | Ty::Inference(_) => {
                        rows.push(
                            Self::decide_entry(
                                Entry::Atom(EffectAtom(interned)),
                                excludeds,
                                engine,
                            )
                            .await
                            .unwrap(),
                        );
                    }

                    _ => panic!(
                        "must be either polyvar, inference, or instance application, found \
                         {interned:?}"
                    ),
                },

                LabelOrType::Label(label) => {
                    // the label may be excluded by one of its own guards
                    rows.extend(Self::decide_entry(Entry::Label(label), excludeds, engine).await);
                }
            }
        }

        rows
    }

    async fn decide_entry(
        entry: Entry,
        mut excludeds: Vec<EffectLabel>,
        engine: &TrackedEngine,
    ) -> Option<GuardedRowEntry> {
        match entry {
            Entry::Label(effect_label) => {
                let mut i = 0;

                while i < excludeds.len() {
                    match effect_label.compare_labels(&excludeds[i], engine).await {
                        // this guard doesn't do anything any more
                        LabelComparison::Apart => {
                            excludeds.swap_remove(i);
                        }

                        // have to keep the excluded label
                        LabelComparison::Blocked | LabelComparison::WaitForInference => {
                            excludeds[i] = excludeds[i].erase_lifetime(engine).await;
                            i += 1;
                        }

                        // this entry got deleted
                        LabelComparison::Same => {
                            // this label is erased from the list, don't add
                            // this to the row
                            return None;
                        }
                    }
                }

                excludeds.sort_unstable();
                excludeds.dedup();

                Some(GuardedRowEntry {
                    source: Entry::Label(effect_label),
                    excluded: engine.intern_unsized(excludeds),
                })
            }

            Entry::Atom(effect_atom) => {
                for excluded in &mut excludeds {
                    *excluded = excluded.erase_lifetime(engine).await;
                }

                excludeds.sort_unstable();
                excludeds.dedup();

                Some(GuardedRowEntry {
                    source: Entry::Atom(effect_atom),
                    excluded: engine.intern_unsized(excludeds),
                })
            }
        }
    }

    pub(crate) async fn fold_with(
        &self,
        folder: &mut impl TyFolder,
        engine: &TrackedEngine,
    ) -> Option<Interned<Ty>> {
        let new_rows = self.rows.fold_with(folder, engine).await?;
        Some(
            Self::new_effect_row(
                new_rows.iter().map(|x| {
                    (
                        match &x.source {
                            Entry::Atom(x) => LabelOrType::Type(x.0.clone()),
                            Entry::Label(x) => LabelOrType::Label(x.clone()),
                        },
                        x.excluded.to_vec(),
                    )
                }),
                engine,
            )
            .await,
        )
    }

    #[must_use]
    pub fn matchings<'x>(&'x self, other: &'x Self) -> Option<impl Iterator<Item = Matching<'x>>> {
        if self.rows.len() != other.rows.len() {
            return None;
        }

        if self
            .rows
            .iter()
            .zip(other.rows.iter())
            .map(|(left, right)| left.matchings(right))
            .all(|x| x.is_some())
        {
            Some(
                self.rows
                    .iter()
                    .zip(other.rows.iter())
                    .flat_map(|(left, right)| left.matchings(right).unwrap()),
            )
        } else {
            None
        }
    }

    #[must_use]
    pub fn guarded_row_entries(&self) -> impl ExactSizeIterator<Item = &GuardedRowEntry> {
        self.rows.iter()
    }

    pub fn interned_iter(&self) -> impl Iterator<Item = &Interned<Ty>> {
        self.rows.iter().flat_map(GuardedRowEntry::interned_iter)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Ty> {
        self.rows.iter().flat_map(GuardedRowEntry::iter)
    }

    #[must_use]
    pub fn has_inference_variable(&self, ty: &Inference) -> bool {
        self.interned_iter().any(|x| x.has_inference_variable(ty))
    }

    #[must_use]
    pub fn is_empty(&self) -> bool { self.rows.is_empty() }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum AtomState {
    Flexible,
    Rigid,
    Stuck,
}

impl EffectLabel {
    #[must_use]
    pub async fn compare_labels(&self, other: &Self, engine: &TrackedEngine) -> LabelComparison {
        if self.effect_symbol_id != other.effect_symbol_id {
            return LabelComparison::Apart;
        }

        let self_args = self.arguments();
        let other_args = other.arguments();

        if self_args.len() != other_args.len() {
            return LabelComparison::Apart;
        }

        let mut comparison = LabelComparison::Same;
        let mut pairs = Vec::new();

        // initialize the pairs of arguments to compare
        for (l, r) in self_args.interned_iter().zip(other_args.interned_iter()) {
            // early exit if the arguments have the same shape
            if Ty::corresponding_lifetimes(l, r, engine).await.is_some() {
                continue;
            }

            pairs.push((l, r));
        }

        while let Some((l, r)) = pairs.pop() {
            // if both are the same, or both are lifetimes, we can skip
            if l == r
                || matches!(
                    (l.kind_of(engine).await, r.kind_of(engine).await),
                    (TyKind::Lifetime, TyKind::Lifetime)
                )
            {
                continue;
            }

            match (&**l, &**r) {
                (Ty::Inference(_), _) | (_, Ty::Inference(_)) => {
                    comparison = comparison.min(LabelComparison::WaitForInference);
                }

                (assoc @ Ty::Application(_), other) | (other, assoc @ Ty::Application(_))
                    if assoc.is_instance_associated() =>
                {
                    // if both happens to be the same associated type with same
                    // shape, we can skip
                    if Ty::corresponding_lifetimes(l, r, engine).await.is_some() {
                        continue;
                    }

                    // if the associated type contains a non-lifetime inference
                    // variable, it may reduce to a type that matches the other
                    // type, so we have to hold back.
                    let this_comparison = if assoc.contains_non_lifetime_inference() {
                        LabelComparison::WaitForInference
                    } else {
                        LabelComparison::Blocked
                    };

                    comparison = comparison.min(this_comparison);
                }

                (Ty::PolyVar(_), _) | (_, Ty::PolyVar(_)) => {
                    comparison = comparison.min(LabelComparison::Blocked);
                }

                (Ty::Application(l_app), Ty::Application(r_app)) => {
                    let Some(args) = l_app.structural_match(r_app) else {
                        return LabelComparison::Apart;
                    };

                    pairs.extend(args);
                }

                // Rows whose labels all match, argument by argument, and whose
                // tails are equal modulo lifetimes.
                (Ty::EffectRow(left_row), Ty::EffectRow(right_row)) => {
                    let labels = left_row.match_labels(right_row);
                    if !labels.is_exact() {
                        return LabelComparison::Apart;
                    }

                    for (left_label, right_label) in labels.matched() {
                        let Some(arguments) = left_label.structural_match(right_label) else {
                            return LabelComparison::Apart;
                        };

                        pairs.extend(arguments);
                    }

                    match (left_row.tail(), right_row.tail()) {
                        (None, None) => {}
                        (Some(left), Some(right)) => pairs.push((left, right)),
                        (None, Some(_)) | (Some(_), None) => return LabelComparison::Apart,
                    }
                }

                (Ty::EffectRow2(_), Ty::EffectRow2(_)) => {
                    // must either `Same` or `Blocked`
                    if Ty::corresponding_lifetimes(l, r, engine).await.is_some() {
                        continue;
                    }

                    comparison = comparison.min(LabelComparison::Blocked);
                }

                _ => return LabelComparison::Apart,
            }
        }

        comparison
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum LabelComparison {
    /// Two labels are distinct for sure, no substitutions can make them equal.
    Apart = 0,

    /// Neither "apart" or "same" can tell for sure, future instantiations of
    /// poly variables or instance associated types reductions will tell.
    Blocked = 1,

    /// The labels contains inference variables, we'll have to wait until the
    /// inference variables are resolved to later determine.
    WaitForInference = 2,

    /// Two labels have the same shape (up to lifetime erasure).
    Same = 3,
}

#[cfg(test)]
mod test;
