use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Identifiable, Query, StableHash, executor, program::Registration,
    storage::intern::Interned,
};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_resolution::resolve_signature;
use rayc_semantic_element::{
    parameter::{Key as ParameterKey, Parameter, ParameterMap},
    return_type::Key as ReturnTypeKey,
};
use rayc_source_file::SourceElement;
use rayc_symbol::{
    GlobalSymbolID,
    source_map::to_absolute_span,
    span::get_span,
    symbol_kind::{SymbolKind, get_symbol_kind},
    syntax::get_def_signature_syntax,
};
use rayc_syntax::def::ParameterEntry;
use rayc_type::{
    poly_var::{Key as PolyVarKey, PolyVarMap},
    ty::{Ty, TyApplicationView},
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
    Resolution(rayc_resolution::Diagnostic),
    InvalidExternSignature(InvalidExternSignature),
}

impl Report for Diagnostic {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        match self {
            Self::Resolution(diagnostic) => diagnostic.report(engine).await,
            Self::InvalidExternSignature(diagnostic) => diagnostic.report(engine).await,
        }
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
pub(crate) enum InvalidExternSignatureKind {
    Polymorphic,
    UnitParameter,
    UnsupportedParameter,
    UnsupportedReturn,
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
pub(crate) struct InvalidExternSignature {
    kind: InvalidExternSignatureKind,
    span: RelativeSpan,
}

impl Report for InvalidExternSignature {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let message = match self.kind {
            InvalidExternSignatureKind::Polymorphic => "an extern definition cannot be polymorphic",
            InvalidExternSignatureKind::UnitParameter => {
                "unit is not allowed in an extern parameter"
            }
            InvalidExternSignatureKind::UnsupportedParameter => {
                "this type is not supported in an extern parameter"
            }
            InvalidExternSignatureKind::UnsupportedReturn => {
                "this type is not supported as an extern return type"
            }
        };
        Rendered::builder()
            .message(message)
            .primary_highlight(Highlight::new(engine.to_absolute_span(&self.span).await, None))
            .build()
    }
}

fn is_c_abi_value_type(ty: &Ty) -> bool {
    match ty {
        Ty::Application(application) => match application.view() {
            TyApplicationView::Primitive(_) => true,
            TyApplicationView::Pointer(pointer) => is_c_abi_value_type(pointer.pointee()),
            TyApplicationView::Tuple(_)
            | TyApplicationView::Lambda(_)
            | TyApplicationView::Error => false,
        },
        Ty::Inference(_) | Ty::PolyVar(_) => false,
    }
}

fn is_unit_type(ty: &Ty) -> bool {
    matches!(ty, Ty::Application(application) if matches!(application.view(), TyApplicationView::Tuple(tuple) if tuple.args().is_empty()))
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

fn build_function_signature(
    engine: &TrackedEngine,
    owner: GlobalSymbolID,
    parameters: Option<&rayc_syntax::def::ParameterList>,
    return_type: Option<&rayc_syntax::def::ReturnType>,
) -> (FunctionSignature, Vec<Diagnostic>) {
    let resolution = resolve_signature(engine, owner, parameters, return_type);
    let mut parameter_map = ParameterMap::new();
    for (ty, span) in resolution.parameters() {
        parameter_map.push(Parameter::builder().span(span).ty(ty.clone()).build());
    }
    let return_type = resolution.return_type().clone();
    let poly_vars = engine.intern(resolution.poly_vars().clone());
    let diagnostics =
        resolution.into_diagnostics().into_iter().map(Diagnostic::Resolution).collect();

    (
        FunctionSignature { parameters: engine.intern(parameter_map), return_type, poly_vars },
        diagnostics,
    )
}

impl Build for Key {
    type Diagnostic = Diagnostic;

    async fn execute(engine: &TrackedEngine, &Self { symbol_id }: &Self) -> Output<Self> {
        let (parameters, return_type) = engine.get_def_signature_syntax(symbol_id).await;
        let (signature, mut diagnostics) =
            build_function_signature(engine, symbol_id, parameters.as_ref(), return_type.as_ref());

        if engine.get_symbol_kind(symbol_id).await == SymbolKind::ExternDef {
            if !signature.poly_vars.is_empty()
                && let Some(span) = engine.get_span(symbol_id).await
            {
                diagnostics.push(Diagnostic::InvalidExternSignature(InvalidExternSignature {
                    kind: InvalidExternSignatureKind::Polymorphic,
                    span,
                }));
            }

            if let Some(parameter_syntax) = parameters.as_ref() {
                for ((_, parameter), entry) in signature.parameters.iter().zip(
                    parameter_syntax.entries().filter_map(|entry| match entry {
                        ParameterEntry::Parameter(parameter) => Some(parameter),
                        ParameterEntry::Ellipsis(_) => None,
                    }),
                ) {
                    let kind = if is_unit_type(parameter.ty()) {
                        Some(InvalidExternSignatureKind::UnitParameter)
                    } else if !is_c_abi_value_type(parameter.ty()) {
                        Some(InvalidExternSignatureKind::UnsupportedParameter)
                    } else {
                        None
                    };
                    if let Some(kind) = kind {
                        diagnostics.push(Diagnostic::InvalidExternSignature(
                            InvalidExternSignature {
                                kind,
                                span: entry.r#type().map_or_else(|| entry.span(), |ty| ty.span()),
                            },
                        ));
                    }
                }
            }

            if !is_unit_type(&signature.return_type)
                && !is_c_abi_value_type(&signature.return_type)
                && let Some(return_syntax) = return_type.as_ref()
            {
                diagnostics.push(Diagnostic::InvalidExternSignature(InvalidExternSignature {
                    kind: InvalidExternSignatureKind::UnsupportedReturn,
                    span: return_syntax
                        .r#type()
                        .map_or_else(|| return_syntax.span(), |ty| ty.span()),
                }));
            }
        }

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
