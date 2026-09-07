use derive_more::From;
use qbice::{Decode, Encode, Identifiable, StableHash};
use rayc_diagnostic::{ByteIndex, Rendered, Report};
use rayc_handler::{Handler, Storage};
use rayc_qbice::TrackedEngine;
use rayc_resolution::resolver::Resolver;
use rayc_semantic_element::return_type::Key;
use rayc_source_file::SourceElement;
use rayc_symbol::{
    symbol_kind::{SymbolKind, get_symbol_kind},
    syntax::get_return_type_syntax,
};
use rayc_type::{poly_var::get_enclosing_poly_var_maps, ty::Ty};

use crate::{
    build::{Build, Output},
    extern_signature::{
        InvalidExternSignature, InvalidExternSignatureKind, is_c_abi_value_type, is_unit_type,
    },
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

impl Build for Key {
    type Diagnostic = Diagnostic;

    async fn execute(engine: &TrackedEngine, &Self { symbol_id }: &Self) -> Output<Self> {
        let syntax = engine.get_return_type_syntax(symbol_id).await;
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
            .build();

        let return_type = if let Some(syntax) = syntax.as_ref() {
            if let Some(syntax) = syntax.r#type() {
                resolver.resolve_type(&syntax).await
            } else {
                Ty::new_star_error(engine)
            }
        } else {
            Ty::new_unit(engine)
        };

        if symbol_kind == SymbolKind::ExternDef
            && !is_unit_type(&return_type)
            && !is_c_abi_value_type(&return_type)
            && let Some(syntax) = syntax.as_ref()
        {
            diagnostics.receive(Diagnostic::InvalidExternSignature(InvalidExternSignature::new(
                InvalidExternSignatureKind::UnsupportedReturn,
                syntax.r#type().map_or_else(|| syntax.span(), |ty| ty.span()),
            )));
        }

        Output::new_with(return_type, diagnostics.into_vec(), obligations.into_vec(), engine)
    }
}

register_build!(Key);
