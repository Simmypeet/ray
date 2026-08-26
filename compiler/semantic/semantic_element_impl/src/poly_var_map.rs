use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_source_file::SourceElement;
use rayc_symbol::{
    source_map::to_absolute_span,
    symbol_kind::{SymbolKind, get_symbol_kind},
    syntax::get_effect_type_parameter_syntax,
};
use rayc_type::{
    poly_var::{PolyVar, PolyVarMap},
    ty::TyKind,
};

use crate::{
    build::{Build, Output},
    function_signature, register_build,
};

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct DuplicatePolyVar {
    name: Interned<str>,
    original_span: RelativeSpan,
    duplicate_span: RelativeSpan,
}

impl Report for DuplicatePolyVar {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .message(format!("duplicate polymorphic variable `{}`", &*self.name))
            .primary_highlight(
                Highlight::builder()
                    .span(engine.to_absolute_span(&self.duplicate_span).await)
                    .message("this polymorphic variable is duplicated")
                    .build(),
            )
            .related(vec![
                Highlight::builder()
                    .span(engine.to_absolute_span(&self.original_span).await)
                    .message("the original polymorphic variable is here")
                    .build(),
            ])
            .build()
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum Diagnostic {
    DuplicatePolyVar(DuplicatePolyVar),
}

impl Report for Diagnostic {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        match self {
            Self::DuplicatePolyVar(diagnostic) => diagnostic.report(engine).await,
        }
    }
}

impl Build for rayc_type::poly_var::Key {
    type Diagnostic = Diagnostic;

    async fn execute(engine: &TrackedEngine, &Self { symbol_id }: &Self) -> Output<Self> {
        let mut diagnostics = Vec::new();
        let poly_vars = match engine.get_symbol_kind(symbol_id).await {
            SymbolKind::Def => {
                return Output::new(
                    engine
                        .query(&function_signature::Key { symbol_id })
                        .await
                        .poly_vars()
                        .cloned()
                        .expect("a definition should own its polymorphic variables"),
                    engine,
                );
            }
            SymbolKind::Effect => {
                let mut poly_vars = PolyVarMap::new();

                if let Some(type_parameters) =
                    engine.get_effect_type_parameter_syntax(symbol_id).await
                {
                    for identifier in type_parameters.parameters() {
                        if let Some(existing_id) = poly_vars.find_by_name(&identifier.kind.0) {
                            let original_span = poly_vars
                                .iter()
                                .find_map(|(id, poly_var)| {
                                    (id == existing_id).then_some(poly_var.span())
                                })
                                .expect("an existing polymorphic variable ID should be valid");
                            diagnostics.push(Diagnostic::DuplicatePolyVar(DuplicatePolyVar {
                                name: identifier.kind.0.clone(),
                                original_span,
                                duplicate_span: identifier.span(),
                            }));
                            continue;
                        }

                        poly_vars.insert(PolyVar::new(
                            identifier.kind.0.clone(),
                            TyKind::Star,
                            identifier.span(),
                        ));
                    }
                }

                poly_vars
            }
            SymbolKind::EffectOperation => {
                panic!("an effect operation does not own a polymorphic-variable map")
            }
            SymbolKind::ExternDef => {
                panic!("an extern definition does not own a polymorphic-variable map")
            }
            SymbolKind::Module => panic!("a module does not own a polymorphic-variable map"),
        };

        Output::new_with(engine.intern(poly_vars), diagnostics, engine)
    }
}

register_build!(rayc_type::poly_var::Key);
