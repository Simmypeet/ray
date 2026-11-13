//! Resolves the paths of the imports of a module.
//!
//! The first segment of an imported path is looked up among the members of
//! the module and the roots of the linked targets, but not among the names
//! that the module imports: an import can't build on another import, so the
//! imports of a module are resolved independently of each other.

use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_handler::Handler;
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_source_file::SourceElement;
use rayc_symbol::{
    GlobalSymbolID,
    member::get_member_by_name,
    source_map::to_absolute_span,
    symbol_kind::{SymbolKind, get_symbol_kind},
};
use rayc_syntax::path::{Path, PathRoot, PathSegment};
use rayc_type::accessibility::is_symbol_accessible_from;

use crate::{
    Diagnostic, InaccessibleSymbol, InvalidThisPath, PathSegmentNotFound, SuperPathInRootModule,
    resolver::{find_module_path_root, find_target_root_by_name},
};

/// Why an import is invalid, apart from naming nothing or something
/// inaccessible.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    qbice::StableHash,
    qbice::Encode,
    qbice::Decode,
)]
pub enum InvalidImportKind {
    /// A segment of the imported path has type or given arguments.
    Arguments,

    /// The path after a `from` clause starts with a keyword, such as
    /// `super`, instead of the name of a member.
    KeywordAfterFrom,

    /// The imported symbol is of a kind that can't be imported.
    NotImportable(SymbolKind),
}

/// An import that is invalid for the reason its kind tells.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    qbice::StableHash,
    qbice::Encode,
    qbice::Decode,
    qbice::Identifiable,
)]
pub struct InvalidImport {
    kind: InvalidImportKind,
    span: RelativeSpan,
}

impl Report for InvalidImport {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let (message, help_message) = match self.kind {
            InvalidImportKind::Arguments => (
                "an imported path cannot have arguments".to_owned(),
                Some("supply the arguments where the imported name is used"),
            ),
            InvalidImportKind::KeywordAfterFrom => {
                ("a path imported from a `from` clause must start with a name".to_owned(), None)
            }
            InvalidImportKind::NotImportable(kind) => (
                format!("{} cannot be imported", kind.str()),
                Some("import its parent instead and name it through the parent"),
            ),
        };

        Rendered::builder()
            .message(message)
            .primary_highlight(Highlight::new(engine.to_absolute_span(&self.span).await, None))
            .maybe_help_message(help_message)
            .build()
    }
}

/// Resolves paths written in the imports of a module, reporting why a path
/// names nothing that can be imported.
#[derive(Clone, Copy)]
pub struct ImportResolver<'a> {
    engine: &'a TrackedEngine,

    /// The module the imports are written in.
    module_id: GlobalSymbolID,

    handler: &'a dyn Handler<Diagnostic>,
}

impl std::fmt::Debug for ImportResolver<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ImportResolver")
            .field("module_id", &self.module_id)
            .finish_non_exhaustive()
    }
}

impl<'a> ImportResolver<'a> {
    /// Creates a resolver of the imports written in the module `module_id`,
    /// reporting to `handler`.
    #[must_use]
    pub const fn new(
        engine: &'a TrackedEngine,
        module_id: GlobalSymbolID,
        handler: &'a dyn Handler<Diagnostic>,
    ) -> Self {
        Self { engine, module_id, handler }
    }

    /// Finds the symbol that the path of a `from` clause names, which the
    /// paths imported after it are relative to.
    pub async fn resolve_from(&self, from: &Path) -> Option<GlobalSymbolID> {
        self.resolve_path(None, from).await
    }

    /// Finds the symbol that the imported `path` names, relative to `parent`,
    /// the symbol its `from` clause names, if it has one.
    pub async fn resolve_import(
        &self,
        parent: Option<GlobalSymbolID>,
        path: &Path,
    ) -> Option<GlobalSymbolID> {
        let symbol_id = self.resolve_path(parent, path).await?;

        // a symbol that is only meaningful through its parent can't be
        // imported
        let kind = self.engine.get_symbol_kind(symbol_id).await;
        if !kind.is_importable() {
            self.report_invalid(InvalidImportKind::NotImportable(kind), path.span());
            return None;
        }

        Some(symbol_id)
    }

    fn report_invalid(&self, kind: InvalidImportKind, span: RelativeSpan) {
        self.handler.receive(Diagnostic::InvalidImport(InvalidImport { kind, span }));
    }

    /// Finds the symbol that `segment` names: a member of `parent` or, for
    /// the first segment of a path without a `from` clause, a member of the
    /// module or the root of a linked target.
    ///
    /// The segment is reported if it names nothing or something inaccessible
    /// from the module. An inaccessible symbol is still returned, so that its
    /// uses are reported as inaccessible rather than as not found.
    async fn resolve_segment(
        &self,
        parent: Option<GlobalSymbolID>,
        segment: &PathSegment,
    ) -> Option<GlobalSymbolID> {
        // the parser reported the missing name
        let identifier = segment.identifier()?;

        // an import names a symbol rather than an instantiation of it
        if let Some(arguments) = segment.arguments() {
            self.report_invalid(InvalidImportKind::Arguments, arguments.span());
        }

        // look the name up
        let name = &identifier.kind.0;
        let symbol_id = match parent {
            Some(parent) => self.engine.get_member_by_name(parent, name).await,
            None => match self.engine.get_member_by_name(self.module_id, name).await {
                Some(symbol_id) => Some(symbol_id),
                None => find_target_root_by_name(self.engine, self.module_id.target_id, name).await,
            },
        };
        let Some(symbol_id) = symbol_id else {
            self.handler.receive(Diagnostic::PathSegmentNotFound(PathSegmentNotFound::new(
                name.clone(),
                identifier.span,
            )));
            return None;
        };

        // the module must be able to see the symbol
        if !self.engine.is_symbol_accessible_from(symbol_id, self.module_id).await {
            self.handler.receive(Diagnostic::InaccessibleSymbol(InaccessibleSymbol::new(
                symbol_id,
                identifier.span,
            )));
        }

        Some(symbol_id)
    }

    /// Finds the symbol that `path` names, starting from `parent` if the
    /// path follows a `from` clause.
    async fn resolve_path(
        &self,
        parent: Option<GlobalSymbolID>,
        path: &Path,
    ) -> Option<GlobalSymbolID> {
        // the root names a symbol on its own, or a member of the parent
        let root = path.root()?;
        let mut symbol_id = match (&root, parent) {
            (PathRoot::Segment(segment), parent) => self.resolve_segment(parent, segment).await?,

            (PathRoot::This(_) | PathRoot::Target(_) | PathRoot::Super(_), Some(_)) => {
                self.report_invalid(InvalidImportKind::KeywordAfterFrom, root.span());
                return None;
            }

            (PathRoot::This(keyword), None) => {
                self.handler
                    .receive(Diagnostic::InvalidThisPath(InvalidThisPath::new(keyword.span)));
                return None;
            }

            (PathRoot::Target(_) | PathRoot::Super(_), None) => {
                let Some(module_id) =
                    find_module_path_root(self.engine, self.module_id, &root).await
                else {
                    self.handler.receive(Diagnostic::SuperPathInRootModule(
                        SuperPathInRootModule::new(root.span()),
                    ));
                    return None;
                };
                module_id
            }
        };

        // each segment after the root names a member of the symbol before it;
        // a `.` without a segment has been reported by the parser
        for part in path.rest() {
            symbol_id = self.resolve_segment(Some(symbol_id), &part.segment()?).await?;
        }

        Some(symbol_id)
    }
}
