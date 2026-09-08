use rayc_handler::Storage;
use rayc_qbice::TrackedEngine;
use rayc_resolution::resolver::Resolver;
use rayc_symbol::syntax::get_type_definition_syntax;
use rayc_type::{poly_var::get_enclosing_poly_var_maps, ty::Ty, type_definition::Key};

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

        let definition = if let Some(syntax) = syntax {
            resolver.resolve_type(&syntax).await
        } else {
            Ty::new_star_error(engine)
        };
        Output::new_with(definition, diagnostics.into_vec(), obligations.into_vec(), engine)
    }
}

register_build!(Key);
