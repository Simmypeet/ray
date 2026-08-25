//! Contains the diagnostics that can be reported while building the
//! symbol table tree.

use std::path::Path;

use bon::Builder;
use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Identifiable, Query, StableHash, executor, program::Registration,
    storage::intern::Interned,
};
use rayc_diagnostic::{Highlight, Rendered, Report};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_source_file::ByteIndex;
use rayc_symbol::{
    GlobalSymbolID,
    name::{get_name, get_qualified_name},
    source_map::to_absolute_span,
    span::get_span,
};
use rayc_target::{TargetID, get_invocation_arguments};

use crate::table;

/// Enumeration of all diagnostics that can be reported while building table
/// tree.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
#[allow(missing_docs)]
pub enum Diagnostic {
    ItemRedefinition(ItemRedefinition),
    SourceFileLoadFail(SourceFileLoadFail),
    InvalidDefDeclaration(InvalidDefDeclaration),
}

impl Report for Diagnostic {
    async fn report(&self, engine: &TrackedEngine) -> rayc_diagnostic::Rendered<ByteIndex> {
        match self {
            Self::ItemRedefinition(diagnostic) => diagnostic.report(engine).await,
            Self::SourceFileLoadFail(diagnostic) => diagnostic.report(engine).await,
            Self::InvalidDefDeclaration(diagnostic) => diagnostic.report(engine).await,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum InvalidDefDeclarationKind {
    ExternHasBody,
    DefMissingBody,
    NonExternVariadic,
    VariadicNotLast,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct InvalidDefDeclaration {
    kind: InvalidDefDeclarationKind,
    span: RelativeSpan,
}

impl InvalidDefDeclaration {
    pub(crate) const fn new(kind: InvalidDefDeclarationKind, span: RelativeSpan) -> Self {
        Self { kind, span }
    }
}

impl Report for InvalidDefDeclaration {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let message = match self.kind {
            InvalidDefDeclarationKind::ExternHasBody => "an extern definition must not have a body",
            InvalidDefDeclarationKind::DefMissingBody => "a non-extern definition must have a body",
            InvalidDefDeclarationKind::NonExternVariadic => {
                "a variadic parameter list is only allowed on an extern definition"
            }
            InvalidDefDeclarationKind::VariadicNotLast => {
                "the variadic marker must be last in the parameter list"
            }
        };
        Rendered::builder()
            .message(message)
            .primary_highlight(Highlight::new(engine.to_absolute_span(&self.span).await, None))
            .build()
    }
}

/// The item symbol with the same name already exists in the given scope.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Builder,
)]
pub struct ItemRedefinition {
    /// The ID of the existing symbol.
    existing_id: GlobalSymbolID,

    /// The span containing the redefinition.
    redefinition_span: RelativeSpan,

    /// The scope in which the duplication occurred.
    in_id: GlobalSymbolID,
}

impl Report for ItemRedefinition {
    async fn report(&self, engine: &TrackedEngine) -> rayc_diagnostic::Rendered<ByteIndex> {
        let existing_symbol_span = engine.get_span(self.existing_id).await;
        let existing_symbol_name = engine.get_name(self.existing_id).await;
        let in_name = engine.get_qualified_name(self.in_id).await;

        let related = if let Some(span) = existing_symbol_span.as_ref() {
            Some(vec![rayc_diagnostic::Highlight::new(
                engine.to_absolute_span(span).await,
                Some(format!("symbol `{}` is already defined here", existing_symbol_name.as_ref())),
            )])
        } else {
            None
        };

        rayc_diagnostic::Rendered::builder()
            .primary_highlight(Highlight::new(
                engine.to_absolute_span(&self.redefinition_span).await,
                Some("redefinition here".to_string()),
            ))
            .message(format!(
                "symbol `{}` is already defined in the scope `{in_name}`",
                existing_symbol_name.as_ref(),
            ))
            .maybe_related(related)
            .build()
    }
}

/// Failed to load source file when building the symbol table.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct SourceFileLoadFail {
    /// The error message from the file loading failure.
    pub error_message: String,

    /// The path to the source file that failed to load.
    pub path: Interned<Path>,

    /// The span of the submodule identifier declaration, if this failure
    /// occurred when loading a submodule. `None` for root file load failures.
    pub submodule_span: Option<RelativeSpan>,
}

impl Report for SourceFileLoadFail {
    async fn report(&self, engine: &TrackedEngine) -> rayc_diagnostic::Rendered<ByteIndex> {
        let (highlight, context_message) = match self.submodule_span.as_ref() {
            Some(submodule_span) => (
                Some(Highlight::new(
                    engine.to_absolute_span(submodule_span).await,
                    Some("submodule declaration here".to_string()),
                )),
                format!("failed to load submodule file `{}`", self.path.display()),
            ),
            None => (None, format!("failed to load root file `{}`", self.path.display())),
        };

        rayc_diagnostic::Rendered::builder()
            .maybe_primary_highlight(highlight)
            .message(format!("{}: {}", context_message, self.error_message))
            .help_message("check if the file exists and is accessible")
            .build()
    }
}

/// A query for retrieving all rendered diagnostics for a target.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Encode, Decode, StableHash, Query,
)]
#[value(Interned<[Rendered<ByteIndex>]>)]
pub struct RenderedKey(pub TargetID);

#[executor(config = Config)]
#[allow(clippy::too_many_lines)]
async fn rendered_executor(
    &RenderedKey(target_id): &RenderedKey,
    engine: &TrackedEngine,
) -> Interned<[Rendered<ByteIndex>]> {
    let arg = engine.get_invocation_arguments(target_id).await;
    let internred_path: Interned<Path> = engine.intern_unsized(arg.file_path().to_path_buf());

    let table = engine.query(&table::Key { target_id }).await;

    let mut rendered = Vec::new();

    if let Ok((_, errors)) =
        engine.query(&rayc_lexical::Key { path: internred_path.clone(), target_id }).await
    {
        for error in errors.iter() {
            rendered.push(error.report(engine).await);
        }
    }

    if let Ok((_, errors)) =
        engine.query(&rayc_syntax::Key { path: internred_path, target_id }).await
    {
        for error in errors.iter() {
            rendered.push(error.report(engine).await);
        }
    }

    for diagnostic in table.diagnostics() {
        rendered.push(diagnostic.report(engine).await);
    }

    engine.intern_unsized(rendered)
}

#[distributed_slice(RAY_PROGRAM)]
static RENDERED_EXECUTOR: Registration<Config> =
    Registration::new::<RenderedKey, RenderedExecutor>();
