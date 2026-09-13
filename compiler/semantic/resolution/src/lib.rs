//! Resolves source type syntax into semantic types.

use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_source_file::SourceElement;
use rayc_symbol::{source_map::to_absolute_span, symbol_kind::SymbolKind};
use rayc_syntax::{
    def::{ParameterEntry, ParameterList, ParameterType},
    effect_row::{EffectRow as EffectRowSyntax, EffectRowAnnotation},
    r#type::Type as TypeSyntax,
};
use rayc_type::{
    poly_var::{PolyVar, PolyVarMap, PolyVarStack},
    ty::TyKind,
};

pub mod obligation;
pub use obligation::{Obligation, PredicateObligation, TraitRefCheck, WfCheck};
pub mod inference;
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
    /// A polymorphic variable was used without being declared by a parameter
    /// type.
    PolyVarNotFound(PolyVarNotFound),
    /// A path segment could not be found in its containing symbol.
    PathSegmentNotFound(PathSegmentNotFound),
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
}

impl Report for Diagnostic {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        match self {
            Self::InvalidThisPath(diagnostic) => diagnostic.report(engine).await,
            Self::NamedTraitTypeProjection(diagnostic) => diagnostic.report(engine).await,
            Self::MissingTraitTypeDeclaration(diagnostic) => diagnostic.report(engine).await,
            Self::ExpectedValueType(diagnostic) => diagnostic.report(engine).await,
            Self::UnsupportedCallableType(diagnostic) => diagnostic.report(engine).await,
            Self::TooManyGivenArguments(diagnostic) => diagnostic.report(engine).await,
            Self::TraitRefCheck(diagnostic) => diagnostic.report(engine).await,
            Self::Predicate(diagnostic) => diagnostic.report(engine).await,
            Self::PolyVarNotFound(diagnostic) => diagnostic.report(engine).await,
            Self::PathSegmentNotFound(diagnostic) => diagnostic.report(engine).await,
            Self::TypeInferenceNotAllowed(diagnostic) => diagnostic.report(engine).await,
            Self::ExplicitTypeArgumentsNotAllowed(diagnostic) => diagnostic.report(engine).await,
            Self::TypeArgumentArityMismatch(diagnostic) => diagnostic.report(engine).await,
            Self::TypeKindMismatch(diagnostic) => diagnostic.report(engine).await,
            Self::ExpectedEffect(diagnostic) => diagnostic.report(engine).await,
            Self::ExpectedTrait(diagnostic) => diagnostic.report(engine).await,
            Self::ExpectedInstance(diagnostic) => diagnostic.report(engine).await,
            Self::PositionalGivenArgumentAfterNamed(diagnostic) => diagnostic.report(engine).await,
            Self::GivenArgumentNotFound(diagnostic) => diagnostic.report(engine).await,
            Self::MissingGivenArgument(diagnostic) => diagnostic.report(engine).await,
            Self::DuplicateGivenArgument(diagnostic) => diagnostic.report(engine).await,
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

/// A polymorphic variable that is not declared by a function parameter type.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct PolyVarNotFound {
    name: Interned<str>,
    span: RelativeSpan,
}

impl Report for PolyVarNotFound {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .primary_highlight(Highlight::new(
                engine.to_absolute_span(&self.span).await,
                Some(format!("type `{}` is not found", &*self.name)),
            ))
            .message(format!("type `{}` is not found", &*self.name))
            .help_message("a polymorphic variable must first appear in a parameter type")
            .build()
    }
}

fn is_poly_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(character) = chars.next() else {
        return false;
    };

    character.is_ascii_lowercase() && chars.next().is_none()
}

fn discover_effect_row_poly_var(
    effect_row: Option<&EffectRowAnnotation>,
    poly_vars: &mut PolyVarMap,
) {
    let variable = match effect_row.and_then(EffectRowAnnotation::effect_row) {
        Some(EffectRowSyntax::Path(path)) => path.bare_identifier(),
        Some(EffectRowSyntax::ConcreteEffectRow(effect_row)) => effect_row
            .tail()
            .and_then(|tail| tail.variable())
            .and_then(|path| path.bare_identifier()),
        None => None,
    };

    if let Some(variable) = variable {
        let _ = poly_vars.insert(PolyVar::new_effect(variable.kind.0.clone(), variable.span()));
    }
}

fn discover_poly_vars(
    ty: &TypeSyntax,
    poly_vars: &mut PolyVarMap,
    poly_var_stack: Option<&PolyVarStack>,
) {
    match ty {
        TypeSyntax::EffectRow(row) => {
            if let Some(variable) =
                row.tail().and_then(|tail| tail.variable()).and_then(|path| path.bare_identifier())
            {
                let _ =
                    poly_vars.insert(PolyVar::new_effect(variable.kind.0.clone(), variable.span()));
            }
        }
        TypeSyntax::Primitive(_) => {}
        TypeSyntax::Pointer(pointer) => {
            if let Some(pointed_type) = pointer.pointed_type() {
                discover_poly_vars(&pointed_type, poly_vars, poly_var_stack);
            }
        }
        TypeSyntax::Tuple(tuple) => {
            for element in tuple.elements() {
                discover_poly_vars(&element, poly_vars, poly_var_stack);
            }
        }
        TypeSyntax::Path(path) => {
            if let Some(identifier) = path.bare_identifier() {
                let existing =
                    poly_var_stack.is_some_and(|x| x.find_by_name(&identifier.kind).is_some());
                if is_poly_var_name(&identifier.kind.0) && !existing {
                    let _ = poly_vars
                        .insert(PolyVar::new_type(identifier.kind.0.clone(), identifier.span()));
                }
            }
            for segment in path.segments() {
                if let Some(arguments) = segment.arguments() {
                    for argument in arguments.type_arguments() {
                        discover_poly_vars(&argument, poly_vars, poly_var_stack);
                    }
                }
            }
        }
    }
}

#[must_use]
pub fn discover_parameter_poly_vars(
    parameters: Option<&ParameterList>,
    poly_var_stack: Option<&PolyVarStack>,
) -> PolyVarMap {
    let mut poly_vars = PolyVarMap::new();

    if let Some(parameters) = parameters {
        for entry in parameters.entries() {
            let ParameterEntry::Parameter(parameter) = entry else { continue };

            if let Some(ty) = parameter.r#type() {
                match ty {
                    ParameterType::Type(ty) => {
                        discover_poly_vars(&ty, &mut poly_vars, poly_var_stack);
                    }
                    ParameterType::CallableSugar(callable) => {
                        if let Some(parameters) = callable.parameters() {
                            for parameter in parameters.parameters() {
                                discover_poly_vars(&parameter, &mut poly_vars, poly_var_stack);
                            }
                        }
                        if let Some(return_type) = callable.return_type()
                            && let Some(return_type) = return_type.r#type()
                        {
                            discover_poly_vars(&return_type, &mut poly_vars, poly_var_stack);
                        }
                        discover_effect_row_poly_var(
                            callable.effect_row().as_ref(),
                            &mut poly_vars,
                        );
                    }
                }
            }
        }
    }

    poly_vars
}

/// Discovers the polymorphic variables declared by a function signature.
#[must_use]
pub fn discover_function_poly_vars(
    parameters: Option<&ParameterList>,
    poly_var_stack: Option<&PolyVarStack>,
) -> PolyVarMap {
    discover_parameter_poly_vars(parameters, poly_var_stack)
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
