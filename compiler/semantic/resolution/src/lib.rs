//! Resolves source type syntax into semantic types.

use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_handler::Handler;
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_source_file::SourceElement;
use rayc_symbol::{GlobalSymbolID, source_map::to_absolute_span, symbol_kind::SymbolKind};
use rayc_syntax::{
    def::{ParameterEntry, ParameterList, ReturnType},
    effect_row::EffectRow as EffectRowSyntax,
    r#type::{Primitive as PrimitiveSyntax, Type as TypeSyntax},
};
use rayc_type::{
    poly_var::{PolyVar, PolyVarMap, PolyVarStack},
    ty::{Mutability, Primitive, Ty, TyKind, args::Args, effect_row::EffectLabel},
};

use crate::path::resolve_path;

pub mod path;

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
    /// A generic symbol was used without its required type arguments.
    MissingTypeArguments(MissingTypeArguments),
    /// A type-argument list has the wrong number of arguments.
    TypeArgumentArityMismatch(TypeArgumentArityMismatch),
    /// An effect-row label resolved to a symbol that is not an effect.
    ExpectedEffect(ExpectedEffect),
}

impl Report for Diagnostic {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        match self {
            Self::PolyVarNotFound(diagnostic) => diagnostic.report(engine).await,
            Self::PathSegmentNotFound(diagnostic) => diagnostic.report(engine).await,
            Self::MissingTypeArguments(diagnostic) => diagnostic.report(engine).await,
            Self::TypeArgumentArityMismatch(diagnostic) => diagnostic.report(engine).await,
            Self::ExpectedEffect(diagnostic) => diagnostic.report(engine).await,
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

/// A generic symbol used without a type-argument list.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct MissingTypeArguments {
    name: Interned<str>,
    span: RelativeSpan,
    expected: usize,
}

impl MissingTypeArguments {
    const fn new(name: Interned<str>, span: RelativeSpan, expected: usize) -> Self {
        Self { name, span, expected }
    }
}

impl Report for MissingTypeArguments {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .primary_highlight(Highlight::new(
                engine.to_absolute_span(&self.span).await,
                Some(format!("expected {} type arguments", self.expected)),
            ))
            .message(format!("missing type arguments for `{}`", &*self.name))
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

#[derive(Debug, Clone)]
pub struct ResolvedParameter {
    pub span: RelativeSpan,
    pub ty: Interned<Ty>,
}

/// The resolved semantic types and polymorphic environment of a function
/// signature.
#[derive(Debug, Clone)]
pub struct SignatureResolution {
    pub parameters: Vec<ResolvedParameter>,
    pub return_type: Interned<Ty>,
}

/// The result of resolving one type against an existing polymorphic
/// environment.
#[derive(Debug, Clone)]
pub struct TypeResolution {
    ty: Interned<Ty>,
}

impl TypeResolution {
    /// Returns the resolved semantic type.
    #[must_use]
    pub const fn ty(&self) -> &Interned<Ty> { &self.ty }
}

fn is_poly_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(character) = chars.next() else {
        return false;
    };

    character.is_ascii_lowercase() && chars.next().is_none()
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
        }
        TypeSyntax::PolymorphicVariable(identifier) => {
            if is_poly_var_name(&identifier.kind.0) {
                poly_vars.insert(PolyVar::new(
                    identifier.kind.0.clone(),
                    TyKind::Star,
                    identifier.span(),
                ));
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

#[allow(clippy::similar_names)]
pub(crate) fn resolve_type(
    engine: &TrackedEngine,
    poly_vars: &PolyVarStack,
    syntax: &TypeSyntax,
    handler: &dyn Handler<Diagnostic>,
) -> Interned<Ty> {
    match syntax {
        TypeSyntax::Primitive(primitive) => {
            let primitive = match primitive {
                PrimitiveSyntax::Int32(_) => Primitive::Int32,
                PrimitiveSyntax::Bool(_) => Primitive::Bool,
                PrimitiveSyntax::Float32(_) => Primitive::Float32,
                PrimitiveSyntax::CInt(_) => Primitive::CInt,
                PrimitiveSyntax::CStr(_) => Primitive::CStr,
            };
            Ty::new_primitive(primitive, engine)
        }
        TypeSyntax::Pointer(pointer) => {
            let pointee = pointer.pointed_type().map_or_else(
                || Ty::new_error(engine),
                |pointed_type| resolve_type(engine, poly_vars, &pointed_type, handler),
            );
            let mutability = if pointer.mut_keyword().is_some() {
                Mutability::Mutable
            } else {
                Mutability::Immutable
            };
            Ty::new_pointer(pointee, mutability, engine)
        }
        TypeSyntax::Tuple(tuple) => {
            let arguments = tuple
                .elements()
                .map(|element| resolve_type(engine, poly_vars, &element, handler))
                .collect::<Vec<_>>();
            Ty::new_tuple(engine.intern_unsized(arguments), engine)
        }
        TypeSyntax::Lambda(lambda) => {
            let parameters = lambda.parameters().map_or_else(Vec::new, |parameters| {
                parameters
                    .parameters()
                    .map(|parameter| resolve_type(engine, poly_vars, &parameter, handler))
                    .collect::<Vec<_>>()
            });
            let return_type = lambda.return_type().map_or_else(
                || Ty::new_unit(engine),
                |return_type| {
                    return_type.r#type().map_or_else(
                        || Ty::new_error(engine),
                        |return_type| resolve_type(engine, poly_vars, &return_type, handler),
                    )
                },
            );
            Ty::new_lambda(parameters, return_type, engine)
        }
        TypeSyntax::PolymorphicVariable(identifier) => {
            if !is_poly_var_name(&identifier.kind.0) {
                return Ty::new_error(engine);
            }

            let Some(id) = poly_vars.find_by_name(&identifier.kind.0) else {
                handler.receive(Diagnostic::PolyVarNotFound(PolyVarNotFound::new(
                    identifier.kind.0.clone(),
                    identifier.span(),
                )));
                return Ty::new_error(engine);
            };

            Ty::new_poly_var(id, engine)
        }
    }
}

fn resolve_effect_row_poly_var(
    engine: &TrackedEngine,
    poly_vars: &PolyVarStack,
    identifier: &rayc_syntax::Identifier,
    handler: &dyn Handler<Diagnostic>,
) -> Option<Interned<Ty>> {
    let Some(id) = poly_vars.find_by_name(&identifier.kind.0) else {
        handler.receive(Diagnostic::PolyVarNotFound(PolyVarNotFound::new(
            identifier.kind.0.clone(),
            identifier.span(),
        )));
        return None;
    };

    Some(Ty::new_poly_var(id, engine))
}

/// Resolves effect-row syntax relative to the closest module containing
/// `site`.
#[must_use]
pub async fn resolve_effect_row(
    engine: &TrackedEngine,
    poly_vars: &PolyVarStack,
    site: GlobalSymbolID,
    syntax: &EffectRowSyntax,
    handler: &dyn Handler<Diagnostic>,
) -> TypeResolution {
    let ty = match syntax {
        EffectRowSyntax::PolyVar(identifier) => {
            resolve_effect_row_poly_var(engine, poly_vars, identifier, handler)
                .unwrap_or_else(|| Ty::new_effect_row([], None, engine))
        }
        EffectRowSyntax::ConcreteEffectRow(effect_row) => {
            let mut labels = Vec::new();

            for path in effect_row.effects() {
                let Ok(path_resolution) =
                    resolve_path(engine, poly_vars, site, &path, handler).await
                else {
                    continue;
                };
                let effect_symbol_id = path_resolution.symbol_id();
                let symbol_kind = path_resolution.symbol_kind(engine).await;
                let arguments = path_resolution
                    .type_arguments()
                    .map_or_else(Vec::new, |arguments| arguments.cloned().collect());

                if symbol_kind != SymbolKind::Effect {
                    handler.receive(Diagnostic::ExpectedEffect(ExpectedEffect::new(
                        path.span(),
                        symbol_kind,
                    )));
                    continue;
                }
                labels.push(
                    engine.intern(EffectLabel::new(effect_symbol_id, Args::new(arguments, engine))),
                );
            }

            let tail = effect_row.tail().and_then(|tail| {
                tail.variable().and_then(|variable| {
                    resolve_effect_row_poly_var(engine, poly_vars, &variable, handler)
                })
            });

            Ty::new_effect_row(labels, tail, engine)
        }
    };

    TypeResolution { ty }
}

/// Resolves a complete function signature against polymorphic variables
/// declared by an enclosing symbol.
#[must_use]
pub fn resolve_signature_with_poly_vars(
    engine: &TrackedEngine,
    parameters: Option<&ParameterList>,
    return_type: Option<&ReturnType>,
    poly_vars: &PolyVarStack,
    handler: &dyn Handler<Diagnostic>,
) -> SignatureResolution {
    let mut resolved_parameters = Vec::new();

    if let Some(parameters) = parameters {
        for entry in parameters.entries() {
            let ParameterEntry::Parameter(parameter) = entry else { continue };
            let ty = parameter.r#type().map_or_else(
                || Ty::new_error(engine),
                |syntax| resolve_type(engine, poly_vars, &syntax, handler),
            );
            resolved_parameters.push(ResolvedParameter { span: parameter.span(), ty });
        }
    }

    let return_type = return_type.map_or_else(
        || Ty::new_unit(engine),
        |return_type| {
            return_type.r#type().map_or_else(
                || Ty::new_error(engine),
                |syntax| resolve_type(engine, poly_vars, &syntax, handler),
            )
        },
    );

    SignatureResolution { parameters: resolved_parameters, return_type }
}

/// Resolves one type against polymorphic variables declared by its owner.
#[must_use]
pub fn resolve_type_with_poly_vars(
    engine: &TrackedEngine,
    poly_vars: &PolyVarStack,
    syntax: &TypeSyntax,
    handler: &dyn Handler<Diagnostic>,
) -> TypeResolution {
    let ty = resolve_type(engine, poly_vars, syntax, handler);
    TypeResolution { ty }
}
