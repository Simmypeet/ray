//! Region renumbering: every lifetime in the IR that is not universal gets its
//! own region, a [`Lifetime::External`] in the interface of a nested function
//! and a [`Lifetime::Region`] anywhere else.

use qbice::storage::intern::Interned;
use rayc_arena::ID;
use rayc_hash::{FxHashMap, FxHashSet};
use rayc_ir::{
    ir_function::{FunctionID, IRFunctionMap},
    ir_lambda::CaptureMapID,
    visit::{TypeSite, TypeVisitorMutAsync},
};
use rayc_qbice::TrackedEngine;
use rayc_type::{
    rewrite::{RewriteAsync, TyRewriterAsync},
    ty::{
        Ty,
        lifetime::{ExternalRegionID, Lifetime},
    },
};

/// The regions created by renumbering the IR functions of a definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Renumbering {
    /// The number of IDs handed out, to regions of either kind.
    id_count: u64,

    /// The external regions created in each capture layout.
    capture_externals: FxHashMap<CaptureMapID, FxHashSet<ExternalRegionID>>,

    /// The external regions created in the signature of each nested function.
    signature_externals: FxHashMap<FunctionID, FxHashSet<ExternalRegionID>>,
}

impl Renumbering {
    /// Replaces every lifetime in `ir` that is not universal with a fresh
    /// region, and returns the regions created.
    #[must_use]
    pub async fn renumber(ir: &mut IRFunctionMap, engine: &TrackedEngine) -> Self {
        let mut renumberer = Renumberer {
            engine,
            id_count: 0,
            capture_externals: FxHashMap::default(),
            signature_externals: FxHashMap::default(),
        };
        ir.visit_types_mut_async(&mut renumberer).await;

        Self {
            id_count: renumberer.id_count,
            capture_externals: renumberer.capture_externals,
            signature_externals: renumberer.signature_externals,
        }
    }

    /// Returns the number of IDs handed out, to regions of either kind.
    #[must_use]
    pub const fn id_count(&self) -> u64 { self.id_count }

    /// Iterates over the external regions created in a capture layout.
    pub fn capture_externals(
        &self,
        capture_map_id: CaptureMapID,
    ) -> impl Iterator<Item = ExternalRegionID> + '_ {
        self.capture_externals.get(&capture_map_id).into_iter().flatten().copied()
    }

    /// Iterates over the external regions created in the signature of a nested
    /// function.
    pub fn signature_externals(
        &self,
        function_id: FunctionID,
    ) -> impl Iterator<Item = ExternalRegionID> + '_ {
        self.signature_externals.get(&function_id).into_iter().flatten().copied()
    }
}

/// The [`TypeVisitorMutAsync`] behind [`Renumbering::renumber`].
struct Renumberer<'e> {
    engine: &'e TrackedEngine,

    /// The number of IDs handed out so far, to regions of either kind.
    id_count: u64,

    /// The external regions created in each capture layout.
    capture_externals: FxHashMap<CaptureMapID, FxHashSet<ExternalRegionID>>,

    /// The external regions created in the signature of each nested function.
    signature_externals: FxHashMap<FunctionID, FxHashSet<ExternalRegionID>>,
}

impl TypeVisitorMutAsync for Renumberer<'_> {
    async fn visit_type_mut_async(&mut self, ty: &mut Interned<Ty>, site: TypeSite) {
        let engine = self.engine;
        let counter = &mut self.id_count;

        match site {
            TypeSite::Capture(capture_map_id) => {
                let created = self.capture_externals.entry(capture_map_id).or_default();
                renumber_lifetimes(ty, || fresh_external(counter, created), engine).await;
            }

            TypeSite::Signature(function_id) => {
                let created = self.signature_externals.entry(function_id).or_default();
                renumber_lifetimes(ty, || fresh_external(counter, created), engine).await;
            }

            TypeSite::Body(_) => {
                renumber_lifetimes(ty, || Lifetime::Region(next_id(counter)), engine).await;
            }
        }
    }
}

/// Returns a fresh [`Lifetime::External`] numbered by `counter`, and records
/// it in `created`.
fn fresh_external(counter: &mut u64, created: &mut FxHashSet<ExternalRegionID>) -> Lifetime {
    let external = next_id(counter);
    created.insert(external);
    Lifetime::External(external)
}

/// Returns the ID numbered by `counter` and advances it.
const fn next_id<T>(counter: &mut u64) -> ID<T> {
    let id = ID::new(*counter);
    *counter += 1;
    id
}

/// Replaces every lifetime in a type that is not universal with a lifetime
/// from `fresh`.
struct LifetimeRenumberer<'e, F> {
    fresh: F,
    engine: &'e TrackedEngine,
}

impl<F: FnMut() -> Lifetime> TyRewriterAsync for LifetimeRenumberer<'_, F> {
    async fn rewrite(&mut self, ty: &Interned<Ty>) -> Option<Interned<Ty>> {
        if ty.is_lifetime(self.engine).await && !ty.is_universal_region() {
            Some(Ty::new_lifetime((self.fresh)(), self.engine))
        } else {
            None
        }
    }
}

/// Replaces every lifetime in `ty` that is not universal, in place, with a
/// fresh lifetime from `fresh`.
async fn renumber_lifetimes(
    ty: &mut Interned<Ty>,
    fresh: impl FnMut() -> Lifetime,
    engine: &TrackedEngine,
) {
    let mut renumberer = LifetimeRenumberer { fresh, engine };
    if let Some(renumbered) = ty.rewrite_async(&mut renumberer, engine).await {
        *ty = renumbered;
    }
}
