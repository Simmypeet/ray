use rayc_handler::Storage;
use rayc_qbice::TrackedEngine;
use rayc_resolution::resolver::Resolver;
use rayc_semantic_element::variant_payload_ty::Key;
use rayc_symbol::syntax::get_variant_payload_syntax;
use rayc_type::poly_var::get_enclosing_poly_var_maps;

use crate::{
    build::{Build, Output},
    register_build,
};

impl Build for Key {
    type Diagnostic = rayc_resolution::Diagnostic;

    async fn execute(engine: &TrackedEngine, &Self { symbol_id }: &Self) -> Output<Self> {
        let Some(syntax) = engine.get_variant_payload_syntax(symbol_id).await else {
            return Output::new(None, engine);
        };

        // A variant owns no polymorphic variables; its payload is resolved in
        // the scope of its enum's type parameters.
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

        let ty = resolver.resolve_type(&syntax).await;

        Output::new_with(Some(ty), diagnostics.into_vec(), obligations.into_vec(), engine)
    }
}

register_build!(Key);
