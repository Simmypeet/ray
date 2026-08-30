//! Semantic path-resolution results.

use qbice::storage::intern::Interned;
use rayc_source_file::SourceElement;
use rayc_symbol::{
    GlobalSymbolID,
    symbol_kind::{SymbolKind, get_symbol_kind},
};
use rayc_syntax::path::{Path, PathSegment};
use rayc_type::{
    poly_var::{GlobalPolyVarID, get_poly_var_map},
    subst::Subst,
    ty::Ty,
};

use crate::resolver::Resolver;

/// The semantic information produced by resolving one path segment.
#[derive(Debug, Clone)]
pub struct PathSegmentResolution {
    symbol_id: GlobalSymbolID,
    type_arguments: Option<Interned<[Interned<Ty>]>>,
}

impl PathSegmentResolution {
    pub(crate) const fn new(
        symbol_id: GlobalSymbolID,
        type_arguments: Option<Interned<[Interned<Ty>]>>,
    ) -> Self {
        Self { symbol_id, type_arguments }
    }

    /// Returns the ID of the symbol resolved by this segment.
    #[must_use]
    pub const fn symbol_id(&self) -> GlobalSymbolID { self.symbol_id }

    /// Iterates over this segment's resolved type arguments.
    #[must_use]
    pub const fn type_arguments(&self) -> Option<&Interned<[Interned<Ty>]>> {
        self.type_arguments.as_ref()
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
    /// The resolved symbol has a different kind than the path context requires.
    UnexpectedSymbolKind,
}

impl PathResolution {
    pub(crate) fn new(segment: PathSegmentResolution) -> Self { Self { segments: vec![segment] } }

    pub(crate) fn push(&mut self, segment: PathSegmentResolution) { self.segments.push(segment); }

    /// Returns the ID of the symbol resolved by the final segment.
    #[must_use]
    pub fn symbol_id(&self) -> GlobalSymbolID {
        self.segments
            .last()
            .expect("a path resolution should contain at least one segment")
            .symbol_id()
    }

    /// Returns the kind of the symbol resolved by the final segment.
    pub async fn symbol_kind(&self, engine: &rayc_qbice::TrackedEngine) -> SymbolKind {
        engine.get_symbol_kind(self.symbol_id()).await
    }

    /// Iterates over the final segment's resolved type arguments.
    #[must_use]
    pub fn type_arguments(&self) -> Option<&Interned<[Interned<Ty>]>> {
        self.segments.last()?.type_arguments()
    }

    /// Iterates over segment resolutions in root-to-final order.
    #[must_use]
    pub fn segments(&self) -> impl ExactSizeIterator<Item = &PathSegmentResolution> {
        self.segments.iter()
    }

    /// Builds the polymorphic substitution introduced by generic path
    /// segments.
    pub async fn substitution(&self, engine: &rayc_qbice::TrackedEngine) -> Subst {
        let mut mappings = Vec::new();

        for segment in &self.segments {
            let Some(arguments) = segment.type_arguments.as_ref() else { continue };
            if !engine.get_symbol_kind(segment.symbol_id).await.has_poly_var_map() {
                continue;
            }
            let poly_vars = engine.get_poly_var_map(segment.symbol_id).await;
            mappings.extend(poly_vars.iter().zip(arguments.iter()).map(
                |((poly_var_id, _), argument)| {
                    (GlobalPolyVarID::new(segment.symbol_id, poly_var_id), argument.clone())
                },
            ));
        }

        mappings.into_iter().collect()
    }

    /// Returns the resolved type arguments attached to `symbol_id`.
    #[must_use]
    pub fn type_arguments_for(
        &self,
        symbol_id: GlobalSymbolID,
    ) -> Option<&Interned<[Interned<Ty>]>> {
        self.segments.iter().find(|segment| segment.symbol_id == symbol_id)?.type_arguments()
    }
}

impl Resolver<'_> {
    /// Resolves a path and requires its final symbol to be an effect.
    pub async fn resolve_effect_path(
        &mut self,
        path: &Path,
    ) -> Result<PathResolution, PathResolutionError> {
        let resolution = self.resolve_path(path).await?;
        let symbol_kind = self.symbol_kind(resolution.symbol_id()).await;

        if symbol_kind != SymbolKind::Effect {
            self.report_expected_effect(path.span(), symbol_kind);
            return Err(PathResolutionError::UnexpectedSymbolKind);
        }

        Ok(resolution)
    }

    /// Resolves every segment in a path from root to final.
    pub async fn resolve_path(
        &mut self,
        path: &Path,
    ) -> Result<PathResolution, PathResolutionError> {
        let mut segments = path.segments();
        let Some(first) = segments.next() else {
            return Err(PathResolutionError::MissingIdentifier);
        };
        let mut resolution = self.resolve_path_segment(&first, None).await?;

        for segment in segments {
            resolution = self.resolve_path_segment(&segment, Some(resolution)).await?;
        }

        Ok(resolution)
    }

    async fn resolve_path_segment(
        &mut self,
        path: &PathSegment,
        previous: Option<PathResolution>,
    ) -> Result<PathResolution, PathResolutionError> {
        let Some(identifier) = path.identifier() else {
            return Err(PathResolutionError::MissingIdentifier);
        };
        let symbol_id = self
            .find_path_symbol(previous.as_ref().map(PathResolution::symbol_id), &identifier.kind.0)
            .await;

        let Some(symbol_id) = symbol_id else {
            self.report_path_segment_not_found(identifier);
            return Err(PathResolutionError::SymbolNotFound);
        };

        let symbol_kind = self.symbol_kind(symbol_id).await;
        let expected = self.poly_var_kinds(symbol_id).await;
        let type_arguments =
            self.resolve_type_arguments(symbol_kind, path, &identifier, &expected).await;
        let segment = PathSegmentResolution::new(symbol_id, type_arguments);

        let Some(mut resolution) = previous else {
            return Ok(PathResolution::new(segment));
        };
        resolution.push(segment);
        Ok(resolution)
    }
}
