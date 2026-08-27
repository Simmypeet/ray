//! Resolves path segments into semantic symbols.

use qbice::storage::intern::Interned;
use rayc_handler::Handler;
use rayc_qbice::TrackedEngine;
use rayc_source_file::SourceElement;
use rayc_symbol::{
    GlobalSymbolID,
    member::{get_member_by_name, try_get_members},
    parent::get_closest_module_id,
    symbol_kind::{SymbolKind, get_symbol_kind},
};
use rayc_syntax::path::{Path, PathSegment};
use rayc_type::{
    poly_var::{PolyVarStack, get_poly_var_map},
    ty::{Ty, TyKind},
};

use crate::{
    Diagnostic, MissingTypeArguments, PathSegmentNotFound, TypeArgumentArityMismatch,
    TypeKindMismatch, resolve_type,
};

/// The semantic information produced by resolving one path segment.
#[derive(Debug, Clone)]
pub struct PathSegmentResolution {
    symbol_id: GlobalSymbolID,
    type_arguments: Option<Interned<[Interned<Ty>]>>,
}

impl PathSegmentResolution {
    /// Returns the ID of the symbol resolved by this segment.
    #[must_use]
    pub const fn symbol_id(&self) -> GlobalSymbolID { self.symbol_id }

    /// Iterates over this segment's resolved type arguments.
    #[must_use]
    pub fn type_arguments(&self) -> Option<impl ExactSizeIterator<Item = &Interned<Ty>>> {
        self.type_arguments.as_ref().map(|arguments| arguments.iter())
    }
}

/// The ordered semantic resolutions for all segments in a path.
#[derive(Debug, Clone)]
pub struct PathResolution {
    segments: Vec<PathSegmentResolution>,
}

/// The reason a path segment could not be resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PathResolutionError {
    /// The path segment has no identifier due to invalid syntax.
    MissingIdentifier,
    /// The identifier does not name a symbol in the searched scope.
    SymbolNotFound,
}

async fn expected_type_argument_kinds(
    engine: &TrackedEngine,
    symbol_id: GlobalSymbolID,
) -> Vec<TyKind> {
    let symbol_kind = engine.get_symbol_kind(symbol_id).await;
    if symbol_kind.has_poly_var_map() {
        engine
            .get_poly_var_map(symbol_id)
            .await
            .iter()
            .map(|(_, poly_var)| poly_var.kind())
            .collect()
    } else {
        Vec::new()
    }
}

async fn resolve_type_arguments(
    engine: &TrackedEngine,
    poly_vars: &PolyVarStack,
    site: GlobalSymbolID,
    path: &PathSegment,
    identifier: &rayc_syntax::Identifier,
    expected: &[TyKind],
    handler: &dyn Handler<Diagnostic>,
) -> Option<Interned<[Interned<Ty>]>> {
    let Some(arguments) = path.type_arguments() else {
        if expected.is_empty() {
            return None;
        }

        handler.receive(Diagnostic::MissingTypeArguments(MissingTypeArguments::new(
            identifier.kind.0.clone(),
            identifier.span(),
            expected.len(),
        )));
        return Some(engine.intern_unsized(
            expected.iter().map(|kind| Ty::new_error(*kind, engine)).collect::<Vec<_>>(),
        ));
    };

    let actual = arguments.arguments().count();
    if actual != expected.len() {
        handler.receive(Diagnostic::TypeArgumentArityMismatch(TypeArgumentArityMismatch::new(
            arguments.span(),
            expected.len(),
            actual,
        )));
    }

    let mut resolved = Vec::new();
    for (index, argument) in arguments.arguments().enumerate() {
        let mut ty = Box::pin(resolve_type(engine, poly_vars, site, &argument, handler)).await;
        if let Some(expected) = expected.get(index) {
            let actual = ty.kind_of(engine).await;
            if actual != *expected {
                handler.receive(Diagnostic::TypeKindMismatch(TypeKindMismatch::new(
                    argument.span(),
                    *expected,
                    actual,
                )));
                ty = Ty::new_error(*expected, engine);
            }
        }
        resolved.push(ty);
    }
    resolved.truncate(expected.len());
    resolved.extend(expected[resolved.len()..].iter().map(|kind| Ty::new_error(*kind, engine)));
    Some(engine.intern_unsized(resolved))
}

impl PathResolution {
    /// Returns the ID of the symbol resolved by the final segment.
    #[must_use]
    pub fn symbol_id(&self) -> GlobalSymbolID {
        self.segments
            .last()
            .expect("a path resolution should contain at least one segment")
            .symbol_id()
    }

    /// Returns the kind of the symbol resolved by the final segment.
    pub async fn symbol_kind(&self, engine: &TrackedEngine) -> SymbolKind {
        engine.get_symbol_kind(self.symbol_id()).await
    }

    /// Iterates over the final segment's resolved type arguments.
    #[must_use]
    pub fn type_arguments(&self) -> Option<impl ExactSizeIterator<Item = &Interned<Ty>>> {
        self.segments.last()?.type_arguments()
    }

    /// Iterates over segment resolutions in root-to-final order.
    #[must_use]
    pub fn segments(&self) -> impl ExactSizeIterator<Item = &PathSegmentResolution> {
        self.segments.iter()
    }
}

/// Resolves one path segment.
///
/// Without a previous resolution, the segment is looked up in the closest
/// module containing `site`. Otherwise, it is looked up in the previous
/// symbol's members.
pub async fn resolve_path_segment(
    engine: &TrackedEngine,
    poly_vars: &PolyVarStack,
    site: GlobalSymbolID,
    path: &PathSegment,
    previous: Option<PathResolution>,
    handler: &dyn Handler<Diagnostic>,
) -> Result<PathResolution, PathResolutionError> {
    let Some(identifier) = path.identifier() else {
        return Err(PathResolutionError::MissingIdentifier);
    };
    let symbol_id = if let Some(previous) = previous.as_ref() {
        let previous_id = previous.symbol_id();
        engine.try_get_members(previous_id).await.and_then(|members| {
            members
                .get_by_name(&identifier.kind.0)
                .map(|member_id| previous_id.target_id.make_global(member_id))
        })
    } else {
        let closest_module_id = engine.get_closest_module_id(site).await;
        let closest_module_id = site.target_id.make_global(closest_module_id);
        engine.get_member_by_name(closest_module_id, &identifier.kind.0).await
    };

    let Some(symbol_id) = symbol_id else {
        handler.receive(Diagnostic::PathSegmentNotFound(PathSegmentNotFound::new(
            identifier.kind.0,
            identifier.span,
        )));
        return Err(PathResolutionError::SymbolNotFound);
    };

    let expected_type_argument_kinds = expected_type_argument_kinds(engine, symbol_id).await;
    let type_arguments = resolve_type_arguments(
        engine,
        poly_vars,
        site,
        path,
        &identifier,
        &expected_type_argument_kinds,
        handler,
    )
    .await;

    let segment = PathSegmentResolution { symbol_id, type_arguments };
    let mut resolution = previous.unwrap_or_else(|| PathResolution { segments: Vec::new() });
    resolution.segments.push(segment);
    Ok(resolution)
}

/// Resolves every segment in a path from root to final.
pub async fn resolve_path(
    engine: &TrackedEngine,
    poly_vars: &PolyVarStack,
    site: GlobalSymbolID,
    path: &Path,
    handler: &dyn Handler<Diagnostic>,
) -> Result<PathResolution, PathResolutionError> {
    let mut segments = path.segments();
    let Some(first) = segments.next() else {
        return Err(PathResolutionError::MissingIdentifier);
    };
    let mut resolution =
        resolve_path_segment(engine, poly_vars, site, &first, None, handler).await?;

    for segment in segments {
        resolution =
            resolve_path_segment(engine, poly_vars, site, &segment, Some(resolution), handler)
                .await?;
    }

    Ok(resolution)
}
