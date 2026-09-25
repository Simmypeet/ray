use std::collections::BTreeSet;

use derive_more::From;
use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_diagnostic::{ByteIndex, Rendered, Report};
use rayc_handler::{Handler, Storage};
use rayc_qbice::TrackedEngine;
use rayc_resolution::{lifetime::LifetimeElision, resolver::Resolver};
use rayc_semantic_element::{parameter::get_parameter_map, return_type::Key};
use rayc_source_file::SourceElement;
use rayc_symbol::{
    GlobalSymbolID,
    symbol_kind::{SymbolKind, get_symbol_kind},
    syntax::get_return_type_syntax,
};
use rayc_type::{
    poly_var::get_enclosing_poly_var_maps,
    ty::{Ty, TyKind},
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

/// Returns the one lifetime the parameter types of `symbol_id` mention, or
/// `None` when they mention none or more than one. Elided lifetimes in the
/// return type stand for it, as in Rust.
async fn sole_parameter_lifetime(
    engine: &TrackedEngine,
    symbol_id: GlobalSymbolID,
) -> Option<Interned<Ty>> {
    let parameters = engine.get_parameter_map(symbol_id).await;
    let mut lifetimes = BTreeSet::new();
    for (_, parameter) in parameters.iter() {
        for ty in Ty::interned_recursive_iter(parameter.ty()) {
            // Errors of kind lifetime count too, so that an unresolved lifetime
            // does not also make elision fail.
            let is_lifetime = ty.kind_of(engine).await == TyKind::Lifetime;
            if is_lifetime {
                lifetimes.insert(ty.clone());
            }
        }
    }

    let mut lifetimes = lifetimes.into_iter();
    let lifetime = lifetimes.next()?;
    lifetimes.next().is_none().then_some(lifetime)
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
            .lifetime_elision(LifetimeElision::Output(
                sole_parameter_lifetime(engine, symbol_id).await,
            ))
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
            && !return_type.is_unit_type()
            && !return_type.is_c_abi_value_type()
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
