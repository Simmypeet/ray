use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Identifiable, Query, StableHash, executor, program::Registration,
    storage::intern::Interned,
};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_semantic_element::{
    parameter::{Key as ParameterKey, Parameter, ParameterMap},
    return_type::Key as ReturnTypeKey,
};
use rayc_source_file::SourceElement;
use rayc_symbol::{
    GlobalSymbolID, MemberID, source_map::to_absolute_span, syntax::get_def_signature_syntax,
};
use rayc_syntax::{
    def::{ParameterList, ReturnType as ReturnTypeSyntax},
    r#type::{Primitive as PrimitiveSyntax, Type as TySyntax},
};
use rayc_type::{
    poly_var::{Key as PolyVarKey, PolyVar, PolyVarMap},
    ty::{Mutability, Primitive, Ty, TyKind},
};

use crate::{
    build::{Build, Output},
    register_build,
};

/// A diagnostic emitted while building a function signature.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub(crate) enum Diagnostic {
    /// A polymorphic variable was not introduced by a parameter type.
    PolyVarNotFound(PolyVarNotFound),
}

impl Report for Diagnostic {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        match self {
            Self::PolyVarNotFound(diagnostic) => diagnostic.report(engine).await,
        }
    }
}

/// A polymorphic variable that is not bound by a parameter type.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub(crate) struct PolyVarNotFound {
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

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub(crate) struct FunctionSignature {
    parameters: Interned<ParameterMap>,
    return_type: Interned<Ty>,
    poly_vars: Interned<PolyVarMap>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<FunctionSignature>)]
pub(crate) struct Key {
    pub(crate) symbol_id: GlobalSymbolID,
}

fn is_poly_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(character) = chars.next() else {
        return false;
    };

    character.is_ascii_lowercase() && chars.next().is_none()
}

fn collect_poly_vars_from_ty(ty: &TySyntax, poly_vars: &mut PolyVarMap) {
    match ty {
        TySyntax::Primitive(_) => {}
        TySyntax::Pointer(pointer) => {
            if let Some(pointed_ty) = pointer.pointed_type() {
                collect_poly_vars_from_ty(&pointed_ty, poly_vars);
            }
        }
        TySyntax::Tuple(tuple) => {
            for element in tuple.elements() {
                collect_poly_vars_from_ty(&element, poly_vars);
            }
        }
        TySyntax::PolymorphicVariable(identifier) => {
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

fn collect_poly_vars(parameters: Option<&ParameterList>) -> PolyVarMap {
    let mut poly_vars = PolyVarMap::new();

    if let Some(parameters) = parameters {
        for parameter in parameters.parameters() {
            if let Some(ty) = parameter.r#type() {
                collect_poly_vars_from_ty(&ty, &mut poly_vars);
            }
        }
    }

    poly_vars
}

#[allow(clippy::similar_names)]
fn resolve_signature_ty(
    engine: &TrackedEngine,
    owner: GlobalSymbolID,
    poly_vars: &PolyVarMap,
    ty: &TySyntax,
    diagnostics: &mut Vec<Diagnostic>,
) -> Interned<Ty> {
    match ty {
        TySyntax::Primitive(primitive) => {
            let primitive = match primitive {
                PrimitiveSyntax::Int32(_) => Primitive::Int32,
                PrimitiveSyntax::Bool(_) => Primitive::Bool,
                PrimitiveSyntax::Float32(_) => Primitive::Float32,
            };

            Ty::new_primitive(primitive, engine)
        }
        TySyntax::Pointer(pointer) => {
            let pointee = pointer.pointed_type().map_or_else(
                || Ty::new_error(engine),
                |pointed_ty| {
                    resolve_signature_ty(engine, owner, poly_vars, &pointed_ty, diagnostics)
                },
            );

            let mutability = if pointer.mut_keyword().is_some() {
                Mutability::Mutable
            } else {
                Mutability::Immutable
            };

            Ty::new_pointer(pointee, mutability, engine)
        }
        TySyntax::Tuple(tuple) => {
            let arguments = tuple
                .elements()
                .map(|element| {
                    resolve_signature_ty(engine, owner, poly_vars, &element, diagnostics)
                })
                .collect::<Vec<_>>();

            Ty::new_tuple(engine.intern_unsized(arguments), engine)
        }
        TySyntax::PolymorphicVariable(identifier) => {
            if !is_poly_var_name(&identifier.kind.0) {
                return Ty::new_error(engine);
            }

            let Some(id) = poly_vars.find_by_name(&identifier.kind.0) else {
                diagnostics.push(Diagnostic::PolyVarNotFound(PolyVarNotFound::new(
                    identifier.kind.0.clone(),
                    identifier.span(),
                )));
                return Ty::new_error(engine);
            };

            Ty::new_poly_var(MemberID::new(owner, id), engine)
        }
    }
}

fn build_parameter_map(
    engine: &TrackedEngine,
    owner: GlobalSymbolID,
    poly_vars: &PolyVarMap,
    parameters: Option<&ParameterList>,
) -> (ParameterMap, Vec<Diagnostic>) {
    let mut parameter_map = ParameterMap::new();
    let mut diagnostics = Vec::new();

    if let Some(parameters) = parameters {
        for parameter in parameters.parameters() {
            let ty = parameter.r#type().map_or_else(
                || Ty::new_error(engine),
                |ty| resolve_signature_ty(engine, owner, poly_vars, &ty, &mut diagnostics),
            );

            parameter_map.push(Parameter::builder().span(parameter.span()).ty(ty).build());
        }
    }

    (parameter_map, diagnostics)
}

fn build_return_type(
    engine: &TrackedEngine,
    owner: GlobalSymbolID,
    poly_vars: &PolyVarMap,
    return_type: Option<&ReturnTypeSyntax>,
) -> (Interned<Ty>, Vec<Diagnostic>) {
    let mut diagnostics = Vec::new();

    let Some(return_type) = return_type else {
        return (Ty::new_unit(engine), diagnostics);
    };

    let return_type = return_type.r#type().map_or_else(
        || Ty::new_error(engine),
        |ty| resolve_signature_ty(engine, owner, poly_vars, &ty, &mut diagnostics),
    );

    (return_type, diagnostics)
}

fn build_function_signature(
    engine: &TrackedEngine,
    owner: GlobalSymbolID,
    parameters: Option<&ParameterList>,
    return_type: Option<&ReturnTypeSyntax>,
) -> (FunctionSignature, Vec<Diagnostic>) {
    let poly_vars = collect_poly_vars(parameters);
    let (parameter_map, mut diagnostics) =
        build_parameter_map(engine, owner, &poly_vars, parameters);
    let (return_type, return_type_diagnostics) =
        build_return_type(engine, owner, &poly_vars, return_type);
    diagnostics.extend(return_type_diagnostics);

    (
        FunctionSignature {
            parameters: engine.intern(parameter_map),
            return_type,
            poly_vars: engine.intern(poly_vars),
        },
        diagnostics,
    )
}

impl Build for Key {
    type Diagnostic = Diagnostic;

    async fn execute(engine: &TrackedEngine, &Self { symbol_id }: &Self) -> Output<Self> {
        let (parameters, return_type) = engine.get_def_signature_syntax(symbol_id).await;
        let (signature, diagnostics) =
            build_function_signature(engine, symbol_id, parameters.as_ref(), return_type.as_ref());

        Output::new_with(engine.intern(signature), diagnostics, engine)
    }
}

register_build!(Key);

#[executor(config = Config, style = qbice::ExecutionStyle::Projection)]
async fn parameter_projection_executor(
    &ParameterKey { symbol_id }: &ParameterKey,
    engine: &TrackedEngine,
) -> Interned<ParameterMap> {
    engine.query(&Key { symbol_id }).await.parameters.clone()
}

#[distributed_slice(RAY_PROGRAM)]
static PARAMETER_PROJECTION_EXECUTOR: Registration<Config> =
    Registration::new::<ParameterKey, ParameterProjectionExecutor>();

#[executor(config = Config, style = qbice::ExecutionStyle::Projection)]
async fn return_type_projection_executor(
    &ReturnTypeKey { symbol_id }: &ReturnTypeKey,
    engine: &TrackedEngine,
) -> Interned<Ty> {
    engine.query(&Key { symbol_id }).await.return_type.clone()
}

#[distributed_slice(RAY_PROGRAM)]
static RETURN_TYPE_PROJECTION_EXECUTOR: Registration<Config> =
    Registration::new::<ReturnTypeKey, ReturnTypeProjectionExecutor>();

#[executor(config = Config, style = qbice::ExecutionStyle::Projection)]
async fn poly_var_projection_executor(
    &PolyVarKey { symbol_id }: &PolyVarKey,
    engine: &TrackedEngine,
) -> Interned<PolyVarMap> {
    engine.query(&Key { symbol_id }).await.poly_vars.clone()
}

#[distributed_slice(RAY_PROGRAM)]
static POLY_VAR_PROJECTION_EXECUTOR: Registration<Config> =
    Registration::new::<PolyVarKey, PolyVarProjectionExecutor>();
