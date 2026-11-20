use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_qbice::TrackedEngine;

use crate::{
    fold::{TyFoldable, TyFolder},
    ty::{Ty, TyKind, application::View, effect_row::EffectLabel, inference::Inference},
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

impl GuardedRowEntry {
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

impl TyFoldable for EffectRow2 {
    async fn fold_with(
        &self,
        _folder: &mut impl TyFolder,
        _engine: &TrackedEngine,
    ) -> Option<Self> {
        todo!("EffectSystem2: fold the atoms and rebuild through a smart constructor")
    }
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
