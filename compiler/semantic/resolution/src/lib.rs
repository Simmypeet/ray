//! Resolves source type syntax into semantic types.

use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_symbol::{
    GlobalSymbolID,
    accessibility::{Accessibility, DeclaredAccessibility, get_declared_accessibility},
    name::{get_name, get_qualified_name},
    source_map::to_absolute_span,
    span::get_span,
    symbol_kind::{SymbolKind, get_symbol_kind},
};
use rayc_type::{accessibility::get_accessibility, ty::TyKind};

pub mod discovery;
pub mod obligation;
pub use obligation::{
    Obligation, PredicateConstraint, PredicateObligation, ReferenceWf, RelationOrigin,
    TraitRefCheck, UnsatisfiedRelationOutlives, WfCheck,
};
pub mod inference;
pub mod lifetime;
pub mod path;
pub use inference::GenInferWithSpan;
pub mod resolver;
pub mod ty;

/// A diagnostic emitted while resolving type syntax.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum Diagnostic {
    /// A `this` path used outside a trait body.
    InvalidThisPath(InvalidThisPath),
    /// A `super` path used in the root module.
    SuperPathInRootModule(SuperPathInRootModule),
    /// An associated type selected through a named trait without a dictionary.
    NamedTraitTypeProjection(NamedTraitTypeProjection),
    /// An instance associated type with no corresponding trait type
    /// declaration.
    MissingTraitTypeDeclaration(MissingTraitTypeDeclaration),
    /// A path resolving to something other than a value type.
    ExpectedValueType(ExpectedValueType),
    UnsupportedCallableType(UnsupportedCallableType),
    TooManyGivenArguments(TooManyGivenArguments),
    /// An explicit instance does not satisfy its given parameter.
    TraitRefCheck(TraitRefCheck),
    /// A resolved symbol's where-clause predicate is not satisfied.
    Predicate(PredicateObligation),
    /// A path segment could not be found in its containing symbol.
    PathSegmentNotFound(PathSegmentNotFound),
    /// A path segment names a symbol that is not accessible from the site.
    InaccessibleSymbol(InaccessibleSymbol),
    /// Resolving omitted type arguments would require disallowed inference.
    TypeInferenceNotAllowed(TypeInferenceNotAllowed),
    /// Explicit type arguments were supplied to a symbol whose parameters are
    /// implicit.
    ExplicitTypeArgumentsNotAllowed(ExplicitTypeArgumentsNotAllowed),
    /// A type-argument list has the wrong number of arguments.
    TypeArgumentArityMismatch(TypeArgumentArityMismatch),
    /// A type has a different kind than its context requires.
    TypeKindMismatch(TypeKindMismatch),
    /// An effect-row label resolved to a symbol that is not an effect.
    ExpectedEffect(ExpectedEffect),
    /// A given parameter's reference resolved to a symbol that is not a trait.
    ExpectedTrait(ExpectedTrait),
    /// A marker implementation's reference resolved to a symbol that is not a
    /// marker.
    ExpectedMarker(ExpectedMarker),
    /// A given argument resolved to a symbol that is not an instance.
    ExpectedInstance(ExpectedInstance),
    /// A positional given argument appeared after a named argument.
    PositionalGivenArgumentAfterNamed(PositionalGivenArgumentAfterNamed),
    /// A named given argument does not correspond to a given parameter.
    GivenArgumentNotFound(GivenArgumentNotFound),
    /// A required given argument was not supplied.
    MissingGivenArgument(MissingGivenArgument),
    /// A given parameter was assigned more than once.
    DuplicateGivenArgument(DuplicateGivenArgument),
    /// A named lifetime is not declared.
    LifetimeNotFound(lifetime::LifetimeNotFound),
    /// A lifetime is elided where elision is not allowed.
    MissingLifetime(lifetime::MissingLifetime),
}

impl Report for Diagnostic {
    // Keep the exhaustive dispatch here so adding a diagnostic forces its
    // rendering path to be selected explicitly.
    #[allow(clippy::cognitive_complexity)]
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        match self {
            Self::InvalidThisPath(diagnostic) => diagnostic.report(engine).await,
            Self::SuperPathInRootModule(diagnostic) => diagnostic.report(engine).await,
            Self::NamedTraitTypeProjection(diagnostic) => diagnostic.report(engine).await,
            Self::MissingTraitTypeDeclaration(diagnostic) => diagnostic.report(engine).await,
            Self::ExpectedValueType(diagnostic) => diagnostic.report(engine).await,
            Self::UnsupportedCallableType(diagnostic) => diagnostic.report(engine).await,
            Self::TooManyGivenArguments(diagnostic) => diagnostic.report(engine).await,
            Self::TraitRefCheck(diagnostic) => diagnostic.report(engine).await,
            Self::Predicate(diagnostic) => diagnostic.report(engine).await,
            Self::PathSegmentNotFound(diagnostic) => diagnostic.report(engine).await,
            Self::InaccessibleSymbol(diagnostic) => diagnostic.report(engine).await,
            Self::TypeInferenceNotAllowed(diagnostic) => diagnostic.report(engine).await,
            Self::ExplicitTypeArgumentsNotAllowed(diagnostic) => diagnostic.report(engine).await,
            Self::TypeArgumentArityMismatch(diagnostic) => diagnostic.report(engine).await,
            Self::TypeKindMismatch(diagnostic) => diagnostic.report(engine).await,
            Self::ExpectedEffect(diagnostic) => diagnostic.report(engine).await,
            Self::ExpectedTrait(diagnostic) => diagnostic.report(engine).await,
            Self::ExpectedMarker(diagnostic) => diagnostic.report(engine).await,
            Self::ExpectedInstance(diagnostic) => diagnostic.report(engine).await,
            Self::PositionalGivenArgumentAfterNamed(diagnostic) => diagnostic.report(engine).await,
            Self::GivenArgumentNotFound(diagnostic) => diagnostic.report(engine).await,
            Self::MissingGivenArgument(diagnostic) => diagnostic.report(engine).await,
            Self::DuplicateGivenArgument(diagnostic) => diagnostic.report(engine).await,
            Self::LifetimeNotFound(diagnostic) => diagnostic.report(engine).await,
            Self::MissingLifetime(diagnostic) => diagnostic.report(engine).await,
        }
    }
}

/// A path segment that is not a member of the preceding symbol.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct PathSegmentNotFound {
    name: Interned<str>,
    span: RelativeSpan,
}

impl PathSegmentNotFound {
    const fn new(name: Interned<str>, span: RelativeSpan) -> Self { Self { name, span } }
}

impl Report for PathSegmentNotFound {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .primary_highlight(Highlight::new(
                engine.to_absolute_span(&self.span).await,
                Some(format!("symbol `{}` is not found", &*self.name)),
            ))
            .message(format!("symbol `{}` is not found", &*self.name))
            .build()
    }
}

/// A symbol referred to from a site it is not accessible from.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    StableHash,
    Encode,
    Decode,
    Identifiable,
)]
pub struct InaccessibleSymbol {
    symbol_id: GlobalSymbolID,
    span: RelativeSpan,
}

impl InaccessibleSymbol {
    /// Creates the diagnostic of `symbol_id` referred to at `span`.
    #[must_use]
    pub const fn new(symbol_id: GlobalSymbolID, span: RelativeSpan) -> Self {
        Self { symbol_id, span }
    }
}

impl Report for InaccessibleSymbol {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let name = engine.get_name(self.symbol_id).await;
        let kind = engine.get_symbol_kind(self.symbol_id).await;

        let declaration = match engine.get_span(self.symbol_id).await {
            Some(span) => vec![Highlight::new(
                engine.to_absolute_span(&span).await,
                Some(format!("`{}` is declared here", &*name)),
            )],
            None => Vec::new(),
        };
        let help_message = match engine.get_accessibility(self.symbol_id).await {
            Accessibility::Public => None,
            Accessibility::Scoped(scope) => {
                let scope = engine.get_qualified_name(scope).await;
                Some(match engine.get_declared_accessibility(self.symbol_id).await {
                    DeclaredAccessibility::Declared(_) => format!(
                        "`{}` is only accessible within `{scope}`; declare it with `pub` to make \
                         it public",
                        &*name
                    ),
                    DeclaredAccessibility::InheritedFromTraitMember => format!(
                        "`{}` has the accessibility of the trait member it implements, which is \
                         only accessible within `{scope}`",
                        &*name
                    ),
                })
            }
        };

        Rendered::builder()
            .primary_highlight(Highlight::new(
                engine.to_absolute_span(&self.span).await,
                Some(format!("`{}` is not accessible here", &*name)),
            ))
            .message(format!("{} `{}` is not accessible here", kind.str(), &*name))
            .related(declaration)
            .maybe_help_message(help_message)
            .build()
    }
}

/// A generic symbol whose omitted type arguments cannot be inferred here.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct TypeInferenceNotAllowed {
    name: Interned<str>,
    span: RelativeSpan,
    expected: usize,
}

impl TypeInferenceNotAllowed {
    const fn new(name: Interned<str>, span: RelativeSpan, expected: usize) -> Self {
        Self { name, span, expected }
    }
}

impl Report for TypeInferenceNotAllowed {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .primary_highlight(Highlight::new(
                engine.to_absolute_span(&self.span).await,
                Some(format!("expected {} type arguments", self.expected)),
            ))
            .message(format!("type inference is not allowed for `{}` here", &*self.name))
            .help_message("provide all type arguments explicitly")
            .build()
    }
}

/// Explicit type arguments supplied to a symbol with implicit type parameters.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    StableHash,
    Encode,
    Decode,
    Identifiable,
)]
pub struct ExplicitTypeArgumentsNotAllowed {
    span: RelativeSpan,
}

impl ExplicitTypeArgumentsNotAllowed {
    const fn new(span: RelativeSpan) -> Self { Self { span } }
}

impl Report for ExplicitTypeArgumentsNotAllowed {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .primary_highlight(Highlight::new(
                engine.to_absolute_span(&self.span).await,
                Some("these type parameters are implicit".into()),
            ))
            .message("explicit type arguments are not allowed for a definition")
            .build()
    }
}

/// A type-argument list whose arity does not match its symbol's parameters.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    StableHash,
    Encode,
    Decode,
    Identifiable,
)]
pub struct TypeArgumentArityMismatch {
    span: RelativeSpan,
    expected: usize,
    actual: usize,
}

impl TypeArgumentArityMismatch {
    const fn new(span: RelativeSpan, expected: usize, actual: usize) -> Self {
        Self { span, expected, actual }
    }
}

impl Report for TypeArgumentArityMismatch {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .primary_highlight(Highlight::new(
                engine.to_absolute_span(&self.span).await,
                Some(format!("expected {}, found {}", self.expected, self.actual)),
            ))
            .message("type argument arity mismatch")
            .build()
    }
}

/// A type whose kind does not match the kind required by its context.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    StableHash,
    Encode,
    Decode,
    Identifiable,
)]
pub struct TypeKindMismatch {
    span: RelativeSpan,
    expected: TyKind,
    actual: TyKind,
}

impl TypeKindMismatch {
    const fn new(span: RelativeSpan, expected: TyKind, actual: TyKind) -> Self {
        Self { span, expected, actual }
    }
}

const fn kind_name(kind: TyKind) -> &'static str {
    match kind {
        TyKind::Star => "a value type",
        TyKind::EffectRow => "an effect row",
        TyKind::Instance => "an instance",
        TyKind::Lifetime => "a lifetime",
    }
}

impl Report for TypeKindMismatch {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .primary_highlight(Highlight::new(
                engine.to_absolute_span(&self.span).await,
                Some(format!(
                    "expected {}, found {}",
                    kind_name(self.expected),
                    kind_name(self.actual)
                )),
            ))
            .message("type kind mismatch")
            .build()
    }
}

/// A symbol used as an effect-row label that is not an effect.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    StableHash,
    Encode,
    Decode,
    Identifiable,
)]
pub struct ExpectedEffect {
    span: RelativeSpan,
    actual: SymbolKind,
}

impl ExpectedEffect {
    const fn new(span: RelativeSpan, actual: SymbolKind) -> Self { Self { span, actual } }
}

impl Report for ExpectedEffect {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .primary_highlight(Highlight::new(
                engine.to_absolute_span(&self.span).await,
                Some(format!("expected an effect, found {}", self.actual.str())),
            ))
            .message(format!("expected an effect, found {}", self.actual.str()))
            .build()
    }
}

/// A symbol used by a given parameter that is not a trait.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    StableHash,
    Encode,
    Decode,
    Identifiable,
)]
pub struct ExpectedTrait {
    span: RelativeSpan,
    actual: SymbolKind,
}

impl ExpectedTrait {
    const fn new(span: RelativeSpan, actual: SymbolKind) -> Self { Self { span, actual } }
}

impl Report for ExpectedTrait {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .primary_highlight(Highlight::new(
                engine.to_absolute_span(&self.span).await,
                Some(format!("expected a trait, found {}", self.actual.str())),
            ))
            .message(format!("expected a trait, found {}", self.actual.str()))
            .build()
    }
}

/// A symbol used by a marker implementation that is not a marker.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    StableHash,
    Encode,
    Decode,
    Identifiable,
)]
pub struct ExpectedMarker {
    span: RelativeSpan,
    actual: SymbolKind,
}

impl ExpectedMarker {
    const fn new(span: RelativeSpan, actual: SymbolKind) -> Self { Self { span, actual } }
}

impl Report for ExpectedMarker {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .primary_highlight(Highlight::new(
                engine.to_absolute_span(&self.span).await,
                Some(format!("expected a marker, found {}", self.actual.str())),
            ))
            .message(format!("expected a marker, found {}", self.actual.str()))
            .build()
    }
}

/// A symbol used as a given argument that is not an instance.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    StableHash,
    Encode,
    Decode,
    Identifiable,
)]
pub struct ExpectedInstance {
    span: RelativeSpan,
    actual: SymbolKind,
}

impl ExpectedInstance {
    const fn new(span: RelativeSpan, actual: SymbolKind) -> Self { Self { span, actual } }
}

impl Report for ExpectedInstance {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .primary_highlight(Highlight::new(
                engine.to_absolute_span(&self.span).await,
                Some(format!("expected an instance, found {}", self.actual.str())),
            ))
            .message(format!("expected an instance, found {}", self.actual.str()))
            .build()
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    StableHash,
    Encode,
    Decode,
    Identifiable,
)]
pub struct PositionalGivenArgumentAfterNamed {
    span: RelativeSpan,
}

impl Report for PositionalGivenArgumentAfterNamed {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .primary_highlight(Highlight::new(
                engine.to_absolute_span(&self.span).await,
                Some("this positional argument appears after a named argument".into()),
            ))
            .message("positional given arguments must appear before named arguments")
            .build()
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct GivenArgumentNotFound {
    name: Interned<str>,
    span: RelativeSpan,
}

impl Report for GivenArgumentNotFound {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .primary_highlight(Highlight::new(
                engine.to_absolute_span(&self.span).await,
                Some(format!("given parameter `{}` is not found", &*self.name)),
            ))
            .message(format!("given parameter `{}` is not found", &*self.name))
            .build()
    }
}

/// A required given argument that was not supplied.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct MissingGivenArgument {
    name: Interned<str>,
    span: RelativeSpan,
}

impl Report for MissingGivenArgument {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .primary_highlight(Highlight::new(
                engine.to_absolute_span(&self.span).await,
                Some(format!("supply the required given argument `{}`", &*self.name)),
            ))
            .message(format!("missing required given argument `{}`", &*self.name))
            .build()
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct DuplicateGivenArgument {
    name: Interned<str>,
    original_span: RelativeSpan,
    duplicate_span: RelativeSpan,
}

impl Report for DuplicateGivenArgument {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .primary_highlight(
                Highlight::builder()
                    .span(engine.to_absolute_span(&self.duplicate_span).await)
                    .message("this given argument is duplicated")
                    .build(),
            )
            .related(vec![
                Highlight::builder()
                    .span(engine.to_absolute_span(&self.original_span).await)
                    .message("the first argument is here")
                    .build(),
            ])
            .message(format!("duplicate given argument `{}`", &*self.name))
            .build()
    }
}

/// A `this` path used outside a trait body.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    StableHash,
    Encode,
    Decode,
    Identifiable,
)]
pub struct InvalidThisPath {
    span: RelativeSpan,
}

impl InvalidThisPath {
    const fn new(span: RelativeSpan) -> Self { Self { span } }
}

impl Report for InvalidThisPath {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .message("`this` is only valid within a trait body")
            .primary_highlight(Highlight::new(engine.to_absolute_span(&self.span).await, None))
            .build()
    }
}

/// A `super` path used in the root module, which has no parent module.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    StableHash,
    Encode,
    Decode,
    Identifiable,
)]
pub struct SuperPathInRootModule {
    span: RelativeSpan,
}

impl SuperPathInRootModule {
    const fn new(span: RelativeSpan) -> Self { Self { span } }
}

impl Report for SuperPathInRootModule {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .message("`super` cannot be used in the root module, which has no parent module")
            .primary_highlight(Highlight::new(engine.to_absolute_span(&self.span).await, None))
            .build()
    }
}

/// An associated type selected through a named trait without a dictionary.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    StableHash,
    Encode,
    Decode,
    Identifiable,
)]
pub struct NamedTraitTypeProjection {
    span: RelativeSpan,
}

impl NamedTraitTypeProjection {
    const fn new(span: RelativeSpan) -> Self { Self { span } }
}

impl Report for NamedTraitTypeProjection {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .message("an associated type requires an instance dictionary, not a named trait")
            .primary_highlight(Highlight::new(engine.to_absolute_span(&self.span).await, None))
            .build()
    }
}

/// An instance associated type with no corresponding trait type declaration.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    StableHash,
    Encode,
    Decode,
    Identifiable,
)]
pub struct MissingTraitTypeDeclaration {
    span: RelativeSpan,
}

impl MissingTraitTypeDeclaration {
    const fn new(span: RelativeSpan) -> Self { Self { span } }
}

impl Report for MissingTraitTypeDeclaration {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .message("associated type has no matching trait type declaration")
            .primary_highlight(Highlight::new(engine.to_absolute_span(&self.span).await, None))
            .build()
    }
}

/// A path resolving to something other than a value type.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    StableHash,
    Encode,
    Decode,
    Identifiable,
)]
pub struct ExpectedValueType {
    span: RelativeSpan,
}

impl ExpectedValueType {
    const fn new(span: RelativeSpan) -> Self { Self { span } }
}

impl Report for ExpectedValueType {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .message("expected a value type or an associated type projection")
            .primary_highlight(Highlight::new(engine.to_absolute_span(&self.span).await, None))
            .build()
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    StableHash,
    Encode,
    Decode,
    Identifiable,
)]
pub struct UnsupportedCallableType {
    span: RelativeSpan,
}
impl Report for UnsupportedCallableType {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .message(
                "callable syntax is only supported as a complete parameter type on ordinary \
                 definitions",
            )
            .primary_highlight(Highlight::new(engine.to_absolute_span(&self.span).await, None))
            .build()
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    StableHash,
    Encode,
    Decode,
    Identifiable,
)]
pub struct TooManyGivenArguments {
    span: RelativeSpan,
    expected: usize,
}
impl Report for TooManyGivenArguments {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .message(format!("only {} explicit given arguments are accepted", self.expected))
            .primary_highlight(Highlight::new(engine.to_absolute_span(&self.span).await, None))
            .build()
    }
}
