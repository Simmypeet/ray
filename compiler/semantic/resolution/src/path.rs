//! Resolves path segments into semantic symbols.

use qbice::storage::intern::Interned;
use rayc_handler::Handler;
use rayc_qbice::TrackedEngine;
use rayc_symbol::{
    GlobalSymbolID,
    member::{get_member_by_name, try_get_members},
    parent::get_closest_module_id,
    symbol_kind::{SymbolKind, get_symbol_kind},
};
use rayc_syntax::path::PathSegment;
use rayc_type::{poly_var::PolyVarStack, ty::Ty};

use crate::{Diagnostic, PathSegmentNotFound, resolve_type};

/// The semantic information produced by resolving one path segment and all of
/// its preceding segments.
#[derive(Debug, Clone)]
pub struct PathResolution {
    symbol_id: Option<GlobalSymbolID>,
    type_arguments: Option<Interned<[Interned<Ty>]>>,
    previous: Option<Box<Self>>,
}

impl PathResolution {
    /// Returns the ID of the symbol resolved by the final segment.
    #[must_use]
    pub const fn symbol_id(&self) -> Option<GlobalSymbolID> { self.symbol_id }

    /// Returns the kind of the symbol resolved by the final segment.
    pub async fn symbol_kind(&self, engine: &TrackedEngine) -> Option<SymbolKind> {
        Some(engine.get_symbol_kind(self.symbol_id?).await)
    }

    /// Iterates over this segment's resolved type arguments.
    #[must_use]
    pub fn type_arguments(&self) -> Option<impl ExactSizeIterator<Item = &Interned<Ty>>> {
        self.type_arguments.as_ref().map(|arguments| arguments.iter())
    }

    /// Iterates from the final segment resolution towards the root segment.
    pub fn recursive_iter(&self) -> impl Iterator<Item = &Self> {
        std::iter::successors(Some(self), |resolution| resolution.previous.as_deref())
    }
}

/// Resolves one path segment.
///
/// Without a previous resolution, the segment is looked up in the closest
/// module containing `site`. Otherwise, it is looked up in the previous
/// symbol's members.
#[must_use]
pub async fn resolve_path(
    engine: &TrackedEngine,
    poly_vars: &PolyVarStack,
    site: GlobalSymbolID,
    path: &PathSegment,
    previous: Option<PathResolution>,
    handler: &dyn Handler<Diagnostic>,
) -> PathResolution {
    let type_arguments = path.type_arguments().map(|arguments| {
        let arguments = arguments
            .arguments()
            .map(|argument| resolve_type(engine, poly_vars, &argument, handler))
            .collect::<Vec<_>>();
        engine.intern_unsized(arguments)
    });

    let identifier = path.identifier();
    let symbol_id = if let Some(identifier) = identifier.as_ref() {
        if let Some(previous_id) = previous.as_ref().and_then(PathResolution::symbol_id) {
            engine.try_get_members(previous_id).await.and_then(|members| {
                members
                    .get_by_name(&identifier.kind.0)
                    .map(|member_id| previous_id.target_id.make_global(member_id))
            })
        } else if previous.is_none() {
            let closest_module_id = engine.get_closest_module_id(site).await;
            let closest_module_id = site.target_id.make_global(closest_module_id);
            engine.get_member_by_name(closest_module_id, &identifier.kind.0).await
        } else {
            None
        }
    } else {
        None
    };

    if symbol_id.is_none()
        && previous.as_ref().is_none_or(|resolution| resolution.symbol_id().is_some())
        && let Some(identifier) = identifier
    {
        handler.receive(Diagnostic::PathSegmentNotFound(PathSegmentNotFound::new(
            identifier.kind.0,
            identifier.span,
        )));
    }

    PathResolution { symbol_id, type_arguments, previous: previous.map(Box::new) }
}
