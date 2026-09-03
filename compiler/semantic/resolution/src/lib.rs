//! Resolves source type syntax into semantic types.

use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_source_file::SourceElement;
use rayc_symbol::{source_map::to_absolute_span, symbol_kind::SymbolKind};
use rayc_syntax::{
    def::{ParameterEntry, ParameterList},
    effect_row::{EffectRow as EffectRowSyntax, EffectRowAnnotation},
    r#type::Type as TypeSyntax,
};
use rayc_type::{
    poly_var::{PolyVar, PolyVarMap},
    ty::TyKind,
};

pub mod path;
pub mod resolver;
pub mod ty;

/// A diagnostic emitted while resolving type syntax.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum Diagnostic {
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
}

impl Report for Diagnostic {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        match self {
            Self::PolyVarNotFound(diagnostic) => diagnostic.report(engine).await,
            Self::PathSegmentNotFound(diagnostic) => diagnostic.report(engine).await,
            Self::TypeInferenceNotAllowed(diagnostic) => diagnostic.report(engine).await,
            Self::ExplicitTypeArgumentsNotAllowed(diagnostic) => diagnostic.report(engine).await,
            Self::TypeArgumentArityMismatch(diagnostic) => diagnostic.report(engine).await,
            Self::TypeKindMismatch(diagnostic) => diagnostic.report(engine).await,
            Self::ExpectedEffect(diagnostic) => diagnostic.report(engine).await,
            Self::ExpectedTrait(diagnostic) => diagnostic.report(engine).await,
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

/// A polymorphic variable that is not declared by a function parameter type.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct PolyVarNotFound {
    name: Interned<str>,
    span: RelativeSpan,
}

impl PolyVarNotFound {
    const fn new(name: Interned<str>, span: RelativeSpan) -> Self { Self { name, span } }
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
        Some(EffectRowSyntax::PolyVar(variable)) => Some(variable),
        Some(EffectRowSyntax::ConcreteEffectRow(effect_row)) => {
            effect_row.tail().and_then(|tail| tail.variable())
        }
        None => None,
    };

    if let Some(variable) = variable {
        poly_vars.insert(PolyVar::new_type(variable.kind.0.clone(), variable.span()));
    }
}

fn discover_poly_vars(ty: &TypeSyntax, poly_vars: &mut PolyVarMap) {
    match ty {
        TypeSyntax::Primitive(_) => {}
        TypeSyntax::Pointer(pointer) => {
            if let Some(pointed_type) = pointer.pointed_type() {
                discover_poly_vars(&pointed_type, poly_vars);
            }
        }
        TypeSyntax::Tuple(tuple) => {
            for element in tuple.elements() {
                discover_poly_vars(&element, poly_vars);
            }
        }
        TypeSyntax::Lambda(lambda) => {
            if let Some(parameters) = lambda.parameters() {
                for parameter in parameters.parameters() {
                    discover_poly_vars(&parameter, poly_vars);
                }
            }
            if let Some(return_type) = lambda.return_type()
                && let Some(return_type) = return_type.r#type()
            {
                discover_poly_vars(&return_type, poly_vars);
            }
            discover_effect_row_poly_var(lambda.effect_row().as_ref(), poly_vars);
        }
        TypeSyntax::PolymorphicVariable(identifier) => {
            if is_poly_var_name(&identifier.kind.0) {
                poly_vars.insert(PolyVar::new_type(identifier.kind.0.clone(), identifier.span()));
            }
        }
    }
}

#[must_use]
pub fn discover_parameter_poly_vars(parameters: Option<&ParameterList>) -> PolyVarMap {
    let mut poly_vars = PolyVarMap::new();

    if let Some(parameters) = parameters {
        for entry in parameters.entries() {
            let ParameterEntry::Parameter(parameter) = entry else { continue };
            if let Some(ty) = parameter.r#type() {
                discover_poly_vars(&ty, &mut poly_vars);
            }
        }
    }

    poly_vars
}

/// Discovers the polymorphic variables declared by a function signature.
#[must_use]
pub fn discover_function_poly_vars(parameters: Option<&ParameterList>) -> PolyVarMap {
    discover_parameter_poly_vars(parameters)
}
