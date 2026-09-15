use derive_more::From;
use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_handler::{Handler, Storage};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_resolution::resolver::Resolver;
use rayc_semantic_element::struct_body::{Field, Key, StructBody};
use rayc_source_file::SourceElement;
use rayc_symbol::{source_map::to_absolute_span, syntax::get_struct_body_syntax};
use rayc_type::{poly_var::get_enclosing_poly_var_maps, ty::Ty};

use crate::{
    build::{Build, Output},
    register_build,
};

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct DuplicateField {
    name: Interned<str>,
    original_span: RelativeSpan,
    duplicate_span: RelativeSpan,
}

impl Report for DuplicateField {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .message(format!("duplicate struct field `{}`", &*self.name))
            .primary_highlight(
                Highlight::builder()
                    .span(engine.to_absolute_span(&self.duplicate_span).await)
                    .message("this field is duplicated")
                    .build(),
            )
            .related(vec![
                Highlight::builder()
                    .span(engine.to_absolute_span(&self.original_span).await)
                    .message("the original field is here")
                    .build(),
            ])
            .build()
    }
}

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
    DuplicateField(DuplicateField),
}

impl Report for Diagnostic {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        match self {
            Self::Resolution(diagnostic) => diagnostic.report(engine).await,
            Self::DuplicateField(diagnostic) => diagnostic.report(engine).await,
        }
    }
}

impl Build for Key {
    type Diagnostic = Diagnostic;

    async fn execute(engine: &TrackedEngine, &Self { symbol_id }: &Self) -> Output<Self> {
        let syntax = engine.get_struct_body_syntax(symbol_id).await;
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
        let mut body = StructBody::new();

        if let Some(syntax) = syntax {
            for field_syntax in syntax.fields() {
                let Some(name) = field_syntax.name() else {
                    continue;
                };
                let ty = if let Some(ty) = field_syntax.r#type() {
                    resolver.resolve_type(&ty).await
                } else {
                    Ty::new_star_error(engine)
                };
                let field =
                    Field::builder().name(name.kind.0.clone()).span(name.span()).ty(ty).build();

                if let Err((duplicate, original_id)) = body.insert(field) {
                    diagnostics.receive(Diagnostic::DuplicateField(DuplicateField {
                        name: duplicate.name().clone(),
                        original_span: body[original_id].span(),
                        duplicate_span: duplicate.span(),
                    }));
                }
            }
        }

        Output::new_with(
            engine.intern(body),
            diagnostics.into_vec(),
            obligations.into_vec(),
            engine,
        )
    }
}

register_build!(Key);
