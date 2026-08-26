use qbice::{Decode, Encode, Identifiable, StableHash};
use rayc_diagnostic::{ByteIndex, Rendered, Report};
use rayc_qbice::TrackedEngine;
use rayc_resolution::{discover_parameter_poly_vars, resolve_type_with_poly_vars};
use rayc_semantic_element::parameter::{Key, Parameter, ParameterMap};
use rayc_source_file::SourceElement;
use rayc_symbol::{
    span::get_span,
    symbol_kind::{SymbolKind, get_symbol_kind},
    syntax::get_parameter_list_syntax,
};
use rayc_syntax::def::ParameterEntry;
use rayc_type::{poly_var::get_enclosing_poly_var_maps, ty::Ty};

use crate::{
    build::{Build, Output},
    extern_signature::{
        InvalidExternSignature, InvalidExternSignatureKind, is_c_abi_value_type, is_unit_type,
    },
    register_build,
};

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
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

impl Build for Key {
    type Diagnostic = Diagnostic;

    async fn execute(engine: &TrackedEngine, &Self { symbol_id }: &Self) -> Output<Self> {
        let syntax = engine.get_parameter_list_syntax(symbol_id).await;
        let symbol_kind = engine.get_symbol_kind(symbol_id).await;
        let poly_vars = engine.get_enclosing_poly_var_maps(symbol_id).await;
        let mut diagnostics = Vec::new();
        let mut parameters = ParameterMap::new();

        if let Some(syntax) = syntax.as_ref() {
            for entry in syntax.entries() {
                let ParameterEntry::Parameter(parameter) = entry else { continue };
                let ty = parameter.r#type().map_or_else(
                    || Ty::new_error(engine),
                    |syntax| {
                        let resolution = resolve_type_with_poly_vars(engine, &poly_vars, &syntax);
                        let ty = resolution.ty().clone();
                        diagnostics.extend(
                            resolution.into_diagnostics().into_iter().map(Diagnostic::Resolution),
                        );
                        ty
                    },
                );
                parameters.push(Parameter::builder().span(parameter.span()).ty(ty).build());
            }
        }

        if symbol_kind == SymbolKind::ExternDef {
            if !discover_parameter_poly_vars(syntax.as_ref()).is_empty()
                && let Some(span) = engine.get_span(symbol_id).await
            {
                diagnostics.push(Diagnostic::InvalidExternSignature(InvalidExternSignature::new(
                    InvalidExternSignatureKind::Polymorphic,
                    span,
                )));
            }

            if let Some(syntax) = syntax.as_ref() {
                for ((_, parameter), entry) in
                    parameters.iter().zip(syntax.entries().filter_map(|entry| match entry {
                        ParameterEntry::Parameter(parameter) => Some(parameter),
                        ParameterEntry::Ellipsis(_) => None,
                    }))
                {
                    let kind = if is_unit_type(parameter.ty()) {
                        Some(InvalidExternSignatureKind::UnitParameter)
                    } else if !is_c_abi_value_type(parameter.ty()) {
                        Some(InvalidExternSignatureKind::UnsupportedParameter)
                    } else {
                        None
                    };
                    if let Some(kind) = kind {
                        diagnostics.push(Diagnostic::InvalidExternSignature(
                            InvalidExternSignature::new(
                                kind,
                                entry.r#type().map_or_else(|| entry.span(), |ty| ty.span()),
                            ),
                        ));
                    }
                }
            }
        }

        Output::new_with(engine.intern(parameters), diagnostics, engine)
    }
}

register_build!(Key);
