use derive_more::From;
use qbice::{Decode, Encode, Identifiable, StableHash};
use rayc_diagnostic::{ByteIndex, Rendered, Report};
use rayc_handler::{Handler, Storage};
use rayc_qbice::TrackedEngine;
use rayc_resolution::{
    discovery::{GivenTraits, discover_parameter_poly_vars},
    lifetime::LifetimeElision,
    resolver::Resolver,
};
use rayc_semantic_element::{
    callable_parameter::get_callable_parameters,
    parameter::{Key, Parameter, ParameterMap},
};
use rayc_source_file::SourceElement;
use rayc_symbol::{
    span::get_span,
    symbol_kind::{SymbolKind, get_symbol_kind},
    syntax::get_parameter_list_syntax,
};
use rayc_syntax::def::{ParameterEntry, ParameterType};
use rayc_type::{
    poly_var::{GlobalPolyVarID, PolyVarOrigin, get_enclosing_poly_var_maps, get_poly_var_map},
    ty::Ty,
};

use crate::{
    build::{Build, Output},
    extern_signature::{InvalidExternSignature, InvalidExternSignatureKind},
    register_build,
};

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    StableHash,
    Encode,
    Decode,
    Identifiable,
    From,
)]
pub enum Diagnostic {
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

/// Returns how the parameter types of a symbol of kind `symbol_kind` treat
/// elided lifetimes. Only a plain `def` owns lifetimes introduced for elision;
/// see `discover_parameter_poly_vars`.
const fn parameter_lifetime_elision(symbol_kind: SymbolKind) -> LifetimeElision {
    match symbol_kind {
        SymbolKind::Def => LifetimeElision::FreshParameter,
        SymbolKind::InstanceDef
        | SymbolKind::TraitDef
        | SymbolKind::ExternDef
        | SymbolKind::EffectOperation
        | SymbolKind::Effect
        | SymbolKind::Instance
        | SymbolKind::MarkerImplementation
        | SymbolKind::Strut
        | SymbolKind::Trait
        | SymbolKind::TraitType
        | SymbolKind::InstanceType
        | SymbolKind::Marker
        | SymbolKind::Module => LifetimeElision::Forbidden,
    }
}

impl Build for Key {
    type Diagnostic = Diagnostic;

    async fn execute(engine: &TrackedEngine, &Self { symbol_id }: &Self) -> Output<Self> {
        let syntax = engine.get_parameter_list_syntax(symbol_id).await;
        let symbol_kind = engine.get_symbol_kind(symbol_id).await;
        let poly_vars = engine.get_enclosing_poly_var_maps(symbol_id).await;
        let diagnostics = Storage::new();
        let obligations = Storage::new();

        let mut resolver = Resolver::builder()
            .engine(engine)
            .poly_var_stack(&poly_vars)
            .site(symbol_id)
            .handler(&diagnostics)
            .obligation_handler(&obligations)
            .lifetime_elision(parameter_lifetime_elision(symbol_kind))
            .build();
        let mut parameters = ParameterMap::new();

        let callables = engine.get_callable_parameters(symbol_id).await;
        if let Some(syntax) = syntax.as_ref() {
            for (index, parameter) in syntax
                .entries()
                .filter_map(|entry| match entry {
                    ParameterEntry::Parameter(parameter) => Some(parameter),
                    ParameterEntry::Ellipsis(_) => None,
                })
                .enumerate()
            {
                // if this position is for a callable sugar parameter, then we need to create a
                // poly var for it
                let ty = if let Some(entry) =
                    callables.iter().find(|entry| entry.occurrence() == index)
                {
                    let map = engine.get_poly_var_map(symbol_id).await;
                    let id = map
                        .find_generated(&PolyVarOrigin::CallableType(entry.occurrence()))
                        .unwrap();
                    Ty::new_poly_var(GlobalPolyVarID::new(symbol_id, id), engine)
                } else if let Some(syntax) = parameter.r#type() {
                    match syntax {
                        ParameterType::Type(ty) => resolver.resolve_type(&ty).await,

                        ParameterType::CallableSugar(callable) => {
                            // Only ordinary definitions can own callable binders.
                            resolver.report_unsupported_callable_type(callable.span());
                            Ty::new_star_error(engine)
                        }
                    }
                } else {
                    Ty::new_star_error(engine)
                };
                parameters.push(Parameter::builder().span(parameter.span()).ty(ty).build());
            }
        }

        if symbol_kind == SymbolKind::ExternDef {
            if !discover_parameter_poly_vars(
                engine,
                symbol_id,
                syntax.as_ref(),
                &GivenTraits::default(),
                Some(&poly_vars),
                false,
            )
            .await
            .is_empty()
                && let Some(span) = engine.get_span(symbol_id).await
            {
                diagnostics.receive(Diagnostic::InvalidExternSignature(
                    InvalidExternSignature::new(InvalidExternSignatureKind::Polymorphic, span),
                ));
            }

            if let Some(syntax) = syntax.as_ref() {
                for ((_, parameter), entry) in
                    parameters.iter().zip(syntax.entries().filter_map(|entry| match entry {
                        ParameterEntry::Parameter(parameter) => Some(parameter),
                        ParameterEntry::Ellipsis(_) => None,
                    }))
                {
                    if parameter.ty().contains_error() {
                        continue;
                    }

                    let kind = if parameter.ty().is_unit_type() {
                        Some(InvalidExternSignatureKind::UnitParameter)
                    } else if !parameter.ty().is_c_abi_value_type() {
                        Some(InvalidExternSignatureKind::UnsupportedParameter)
                    } else {
                        None
                    };

                    if let Some(kind) = kind {
                        diagnostics.receive(Diagnostic::InvalidExternSignature(
                            InvalidExternSignature::new(
                                kind,
                                entry.r#type().map_or_else(|| entry.span(), |ty| ty.span()),
                            ),
                        ));
                    }
                }
            }
        }

        Output::new_with(
            engine.intern(parameters),
            diagnostics.into_vec(),
            obligations.into_vec(),
            engine,
        )
    }
}

register_build!(Key);
