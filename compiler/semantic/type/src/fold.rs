//! Structural rewriting of the types nested in a value, shared by
//! substitution, reduction by givens, and lifetime rewriting.

use qbice::{Identifiable, StableHash, storage::intern::Interned};
use rayc_qbice::TrackedEngine;

use crate::ty::Ty;

/// Decides which nested types a [`TyFoldable::fold_with`] replaces.
pub trait TyFolder: Sized {
    /// Returns the replacement for `ty`, or `None` to keep it.
    ///
    /// The default descends into `ty`; an override chooses the traversal
    /// order by when it calls [`Ty::super_fold_with`], if at all.
    #[allow(async_fn_in_trait)]
    async fn fold_ty(&mut self, ty: &Interned<Ty>, engine: &TrackedEngine) -> Option<Interned<Ty>> {
        Ty::super_fold_with(ty, self, engine).await
    }
}

/// A value whose nested types a [`TyFolder`] can replace.
///
/// Rebuilding goes through the smart constructors, so a fold may reduce what
/// its replacements made reducible.
pub trait TyFoldable: Sized {
    /// Folds every type nested in `self`, or returns `None` when nothing
    /// changes.
    #[must_use]
    #[allow(async_fn_in_trait)]
    async fn fold_with(&self, folder: &mut impl TyFolder, engine: &TrackedEngine) -> Option<Self>;

    /// Like [`TyFoldable::fold_with`], but returns a clone of `self` when
    /// nothing changes.
    #[must_use]
    #[allow(async_fn_in_trait)]
    async fn fold_or_clone(&self, folder: &mut impl TyFolder, engine: &TrackedEngine) -> Self
    where
        Self: Clone,
    {
        self.fold_with(folder, engine).await.unwrap_or_else(|| self.clone())
    }
}

impl TyFoldable for Interned<Ty> {
    async fn fold_with(&self, folder: &mut impl TyFolder, engine: &TrackedEngine) -> Option<Self> {
        folder.fold_ty(self, engine).await
    }
}

impl Ty {
    /// Folds the types nested in `ty` and rebuilds it through the smart
    /// constructors, or returns `None` when none of them changes.
    pub async fn super_fold_with(
        ty: &Interned<Self>,
        folder: &mut impl TyFolder,
        engine: &TrackedEngine,
    ) -> Option<Interned<Self>> {
        // The nested folds recurse back into the folder, so their futures are
        // boxed.
        match &**ty {
            Self::Application(application) => Box::pin(application.fold_args(folder, engine)).await,
            Self::EffectRow(row) => Box::pin(row.fold_with(folder, engine)).await,
            Self::EffectRow2(row) => Box::pin(row.fold_with(folder, engine))
                .await
                .map(|row| engine.intern(Self::EffectRow2(row))),

            Self::Inference(_) | Self::PolyVar(_) | Self::SelfInstance(_) | Self::Lifetime(_) => {
                None
            }
        }
    }
}

impl<T: TyFoldable + Clone + StableHash + Identifiable + Send + Sync + 'static> TyFoldable
    for Interned<[T]>
{
    async fn fold_with(&self, folder: &mut impl TyFolder, engine: &TrackedEngine) -> Option<Self> {
        let mut new_vec = None;

        // Copy the elements only once the first one changes.
        for (i, element) in self.iter().enumerate() {
            match (new_vec.as_mut(), element.fold_with(folder, engine).await) {
                (None, Some(new_element)) => {
                    let mut vec = self.to_vec();
                    vec[i] = new_element;
                    new_vec = Some(vec);
                }
                (Some(vec), Some(new_element)) => vec[i] = new_element,
                (_, None) => {}
            }
        }

        new_vec.map(|vec| engine.intern_unsized(vec))
    }
}

/// Folds both parts of a pair, or returns `None` when neither changes.
pub async fn fold_pair<A: TyFoldable + Clone, B: TyFoldable + Clone>(
    first: &A,
    second: &B,
    folder: &mut impl TyFolder,
    engine: &TrackedEngine,
) -> Option<(A, B)> {
    let new_first = first.fold_with(folder, engine).await;
    let new_second = second.fold_with(folder, engine).await;
    if new_first.is_none() && new_second.is_none() {
        return None;
    }

    Some((new_first.unwrap_or_else(|| first.clone()), new_second.unwrap_or_else(|| second.clone())))
}
