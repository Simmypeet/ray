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
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine, unordered::query_all};
use rayc_source_file::ByteIndex;
use rayc_symbol::{
    GlobalSymbolID,
    name::{get_name, get_qualified_name},
    source_map::to_absolute_span,
    span::get_span,
};
use rayc_target::TargetID;

use crate::{
    index::get_table_index,
    table::{TableKey, get_table},
};

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
    InvalidEffectOperationDeclaration(InvalidEffectOperationDeclaration),
    InvalidAttribute(InvalidAttribute),
    InvalidAccessModifier(InvalidAccessModifier),
}

impl Report for Diagnostic {
    async fn report(&self, engine: &TrackedEngine) -> rayc_diagnostic::Rendered<ByteIndex> {
        match self {
            Self::ItemRedefinition(diagnostic) => diagnostic.report(engine).await,
            Self::SourceFileLoadFail(diagnostic) => diagnostic.report(engine).await,
            Self::InvalidDefDeclaration(diagnostic) => diagnostic.report(engine).await,
            Self::InvalidEffectOperationDeclaration(diagnostic) => diagnostic.report(engine).await,
            Self::InvalidAttribute(diagnostic) => diagnostic.report(engine).await,
            Self::InvalidAccessModifier(diagnostic) => diagnostic.report(engine).await,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum InvalidAttributeKind {
    Unknown,
    Duplicated,
}

/// An attribute that the declaration does not accept, or that is repeated.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct InvalidAttribute {
    kind: InvalidAttributeKind,
    name: Interned<str>,
    span: RelativeSpan,
}

impl InvalidAttribute {
    pub(crate) const fn new(
        kind: InvalidAttributeKind,
        name: Interned<str>,
        span: RelativeSpan,
    ) -> Self {
        Self { kind, name, span }
    }
}

impl Report for InvalidAttribute {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let message = match self.kind {
            InvalidAttributeKind::Unknown => format!("unknown attribute `@{}`", self.name.as_ref()),
            InvalidAttributeKind::Duplicated => {
                format!("attribute `@{}` is specified more than once", self.name.as_ref())
            }
        };
        Rendered::builder()
            .message(message)
            .primary_highlight(Highlight::new(engine.to_absolute_span(&self.span).await, None))
            .build()
    }
}

/// The kind of declaration that can't have an access modifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum InvalidAccessModifierKind {
    /// An instance member has the accessibility of the trait member it
    /// implements.
    InstanceMember,

    /// A marker implementation has no name to be referred to by.
    MarkerImplementation,
}

/// An access modifier written on a declaration that can't have one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct InvalidAccessModifier {
    kind: InvalidAccessModifierKind,
    span: RelativeSpan,
}

impl InvalidAccessModifier {
    pub(crate) const fn new(kind: InvalidAccessModifierKind, span: RelativeSpan) -> Self {
        Self { kind, span }
    }
}

impl Report for InvalidAccessModifier {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let (message, help_message) = match self.kind {
            InvalidAccessModifierKind::InstanceMember => (
                "an instance member cannot have an access modifier",
                "an instance member has the accessibility of the trait member it implements",
            ),
            InvalidAccessModifierKind::MarkerImplementation => (
                "a marker implementation cannot have an access modifier",
                "a marker implementation applies wherever its marker is accessible",
            ),
        };
        Rendered::builder()
            .message(message)
            .primary_highlight(Highlight::new(engine.to_absolute_span(&self.span).await, None))
            .help_message(help_message)
            .build()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct InvalidEffectOperationDeclaration {
    span: RelativeSpan,
}

impl InvalidEffectOperationDeclaration {
    pub(crate) const fn new(span: RelativeSpan) -> Self { Self { span } }
}

impl Report for InvalidEffectOperationDeclaration {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .message("an effect operation cannot have a variadic parameter")
            .primary_highlight(Highlight::new(engine.to_absolute_span(&self.span).await, None))
            .build()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum InvalidDefDeclarationKind {
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

/// A query for retrieving the rendered diagnostics of a single source file of
/// a target: its lexical and syntax errors and the errors found while building
/// its table.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Encode, Decode, StableHash, Query)]
#[value(Interned<[Rendered<ByteIndex>]>)]
struct FileRenderedKey(Interned<TableKey>);

#[executor(config = Config)]
async fn file_rendered_executor(
    FileRenderedKey(table_key): &FileRenderedKey,
    engine: &TrackedEngine,
) -> Interned<[Rendered<ByteIndex>]> {
    let path = table_key.path().clone();
    let target_id = table_key.target_id();

    let mut rendered = Vec::new();

    // the lexical and syntax errors of the file
    if let Ok((_, errors)) =
        engine.query(&rayc_lexical::Key { path: path.clone(), target_id }).await
    {
        for error in errors.iter() {
            rendered.push(error.report(engine).await);
        }
    }

    if let Ok((_, errors)) = engine.query(&rayc_syntax::Key { path, target_id }).await {
        for error in errors.iter() {
            rendered.push(error.report(engine).await);
        }
    }

    // the errors found while building the table of the file
    for diagnostic in engine.get_table(table_key).await.diagnostics() {
        rendered.push(diagnostic.report(engine).await);
    }

    engine.intern_unsized(rendered)
}

#[distributed_slice(RAY_PROGRAM)]
static FILE_RENDERED_EXECUTOR: Registration<Config> =
    Registration::new::<FileRenderedKey, FileRenderedExecutor>();

/// A query for retrieving all rendered diagnostics for a target.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Encode, Decode, StableHash, Query,
)]
#[value(Interned<[Rendered<ByteIndex>]>)]
pub struct RenderedKey(pub TargetID);

#[executor(config = Config)]
async fn rendered_executor(
    &RenderedKey(target_id): &RenderedKey,
    engine: &TrackedEngine,
) -> Interned<[Rendered<ByteIndex>]> {
    let index = engine.get_table_index(target_id).await;

    // the diagnostics of each source file are independent from the others
    let rendered_by_file = engine.query_all(index.table_keys().cloned().map(FileRenderedKey)).await;

    let rendered =
        rendered_by_file.iter().flat_map(|rendered| rendered.iter().cloned()).collect::<Vec<_>>();

    engine.intern_unsized(rendered)
}

#[distributed_slice(RAY_PROGRAM)]
static RENDERED_EXECUTOR: Registration<Config> =
    Registration::new::<RenderedKey, RenderedExecutor>();
