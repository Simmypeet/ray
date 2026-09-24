//! Structural rewriting of types.
//!
//! A [`TyRewriter`] decides, type by type, whether to replace a type. A
//! [`Rewrite`] walks a value depth-first, asks the rewriter about every type
//! it meets, and rebuilds only the parts that change.

use qbice::{Identifiable, StableHash, storage::intern::Interned};
use rayc_qbice::TrackedEngine;

use crate::ty::Ty;

/// Decides which types a [`Rewrite`] replaces.
pub trait TyRewriter {
    /// Returns the replacement for `ty`, or `None` to keep `ty` and rewrite
    /// the types nested in it instead.
    ///
    /// A replacement is used as is: the types nested in it are not rewritten.
    fn rewrite(&mut self, ty: &Interned<Ty>) -> Option<Interned<Ty>>;
}

/// A value whose nested types can be rewritten by a [`TyRewriter`].
pub trait Rewrite {
    /// Rewrites the types in `self` depth-first, visiting a type before the
    /// types nested in it, and returns the rewritten value, or `None` when
    /// nothing changes.
    ///
    /// The value is rebuilt lazily: only the parts that contain a replaced
    /// type are reconstructed.
    #[must_use]
    fn rewrite(&self, rewriter: &mut impl TyRewriter, engine: &TrackedEngine) -> Option<Self>
    where
        Self: Sized;

    /// Like [`Rewrite::rewrite`], but returns a clone of `self` when nothing
    /// changes.
    #[must_use]
    fn rewrite_or_clone(&self, rewriter: &mut impl TyRewriter, engine: &TrackedEngine) -> Self
    where
        Self: Sized + Clone,
    {
        self.rewrite(rewriter, engine).unwrap_or_else(|| self.clone())
    }
}

impl Rewrite for Interned<Ty> {
    fn rewrite(&self, rewriter: &mut impl TyRewriter, engine: &TrackedEngine) -> Option<Self> {
        // The rewriter sees the type before anything nested in it.
        if let Some(replacement) = rewriter.rewrite(self) {
            return Some(replacement);
        }

        match &**self {
            Ty::Application(application) => application
                .rewrite(rewriter, engine)
                .map(|application| engine.intern(Ty::Application(application))),
            Ty::EffectRow(row) => {
                row.rewrite(rewriter, engine).map(|row| engine.intern(Ty::EffectRow(row)))
            }
            Ty::Inference(_) | Ty::PolyVar(_) | Ty::SelfInstance(_) | Ty::Lifetime(_) => None,
        }
    }
}

impl<T: Rewrite + Clone + StableHash + Identifiable + Send + Sync + 'static> Rewrite
    for Interned<[T]>
{
    fn rewrite(&self, rewriter: &mut impl TyRewriter, engine: &TrackedEngine) -> Option<Self> {
        let mut new_vec = None;

        // Copy the elements only once the first one changes.
        for (i, element) in self.iter().enumerate() {
            match (new_vec.as_mut(), element.rewrite(rewriter, engine)) {
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
