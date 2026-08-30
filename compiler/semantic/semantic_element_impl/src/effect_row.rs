use derive_more::From;
use qbice::{Decode, Encode, Identifiable, StableHash};
use rayc_diagnostic::{ByteIndex, Rendered, Report};
use rayc_handler::Storage;
use rayc_qbice::TrackedEngine;
use rayc_resolution::resolver::Resolver;
use rayc_semantic_element::effect_row::Key;
use rayc_symbol::syntax::get_effect_row_syntax;
use rayc_type::{
    poly_var::get_enclosing_poly_var_maps,
    ty::{Ty, TyKind},
};

use crate::{
    build::{Build, Output},
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
}

impl Report for Diagnostic {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        match self {
            Self::Resolution(diagnostic) => diagnostic.report(engine).await,
        }
    }
}

impl Build for Key {
    type Diagnostic = Diagnostic;

    async fn execute(engine: &TrackedEngine, &Self { symbol_id }: &Self) -> Output<Self> {
        let syntax = engine.get_effect_row_syntax(symbol_id).await;
        let poly_vars = engine.get_enclosing_poly_var_maps(symbol_id).await;
        let diagnostics = Storage::new();
        let mut resolver = Resolver::new(engine, &poly_vars, symbol_id, &diagnostics, None);

        let effect_row = match syntax
            .as_ref()
            .and_then(rayc_syntax::effect_row::EffectRowAnnotation::effect_row)
        {
            Some(syntax) => resolver.resolve_effect_row(&syntax).await,
            None if syntax.is_some() => Ty::new_error(TyKind::EffectRow, engine),
            None => Ty::new_effect_row([], None, engine),
        };

        Output::new_with(effect_row, diagnostics.into_vec(), engine)
    }
}

register_build!(Key);
