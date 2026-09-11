use rayc_handler::Storage;
use rayc_qbice::TrackedEngine;
use rayc_resolution::resolver::Resolver;
use rayc_symbol::{
    symbol_kind::{SymbolKind, get_symbol_kind},
    syntax::get_type_definition_syntax,
};
use rayc_type::{
    associated_type_kind::get_associated_type_kind,
    instance_member::get_instance_member,
    poly_var::get_enclosing_poly_var_maps,
    ty::{Ty, TyKind},
    type_definition::Key,
};

use crate::{
    build::{Build, Output},
    register_build,
};

impl Build for Key {
    type Diagnostic = rayc_resolution::Diagnostic;

    async fn execute(engine: &TrackedEngine, &Self { symbol_id }: &Self) -> Output<Self> {
        let syntax = engine.get_type_definition_syntax(symbol_id).await;
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

        // The trait declaration fixes the result kind independently of the
        // implementation.
        let expected = if let Some(member) = engine.get_instance_member(symbol_id).await {
            if engine.get_symbol_kind(member.trait_member_id()).await == SymbolKind::TraitType {
                engine.get_associated_type_kind(member.trait_member_id()).await
            } else {
                TyKind::Star
            }
        } else {
            TyKind::Star
        };
        let definition = if let Some(syntax) = syntax {
            resolver.resolve_type_term(&syntax, expected).await
        } else {
            Ty::new_error(expected, engine)
        };
        Output::new_with(definition, diagnostics.into_vec(), obligations.into_vec(), engine)
    }
}

register_build!(Key);
