use derive_more::From;
use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_handler::{Handler, Storage};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_resolution::resolver::Resolver;
use rayc_semantic_element::instance_trait_ref::Key;
use rayc_symbol::{
    member::{get_member_by_name, get_members},
    name::get_name,
    source_map::to_absolute_span,
    span::get_span,
    syntax::get_instance_trait_syntax,
};
use rayc_type::poly_var::get_enclosing_poly_var_maps;

use crate::{
    build::{Build, Output},
    register_build,
};

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct MissingDefinition {
    name: Interned<str>,
    instance_span: RelativeSpan,
    trait_def_span: RelativeSpan,
}

impl Report for MissingDefinition {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .message(format!("instance is missing trait method `{}`", &*self.name))
            .primary_highlight(
                Highlight::builder()
                    .span(engine.to_absolute_span(&self.instance_span).await)
                    .message(format!("implement `{}` in this instance", &*self.name))
                    .build(),
            )
            .related(vec![
                Highlight::builder()
                    .span(engine.to_absolute_span(&self.trait_def_span).await)
                    .message("the trait method is declared here")
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
    MissingDefinition(MissingDefinition),
}

impl Report for Diagnostic {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        match self {
            Self::Resolution(diagnostic) => diagnostic.report(engine).await,
            Self::MissingDefinition(diagnostic) => diagnostic.report(engine).await,
        }
    }
}

impl Build for Key {
    type Diagnostic = Diagnostic;

    async fn execute(engine: &TrackedEngine, &Self { symbol_id }: &Self) -> Output<Self> {
        let diagnostics = Storage::new();
        let Some(syntax) = engine.get_instance_trait_syntax(symbol_id).await else {
            return Output::new_with(None, diagnostics.into_vec(), engine);
        };
        let poly_vars = engine.get_enclosing_poly_var_maps(symbol_id).await;

        let mut resolver = Resolver::builder()
            .engine(engine)
            .poly_var_stack(&poly_vars)
            .site(symbol_id)
            .handler(&diagnostics)
            .build();
        let Ok(trait_ref) = resolver.resolve_trait_path(&syntax).await else {
            return Output::new_with(None, diagnostics.into_vec(), engine);
        };

        let instance_span =
            engine.get_span(symbol_id).await.expect("an instance symbol should have a source span");
        let trait_members = engine.get_members(trait_ref.trait_id()).await;

        let mut trait_definitions = Vec::new();
        for trait_def_id in trait_members.namable_members() {
            let trait_def_id = trait_ref.trait_id().target_id.make_global(trait_def_id);
            trait_definitions.push((engine.get_name(trait_def_id).await, trait_def_id));
        }

        for (name, trait_def_id) in trait_definitions {
            let trait_def_span = engine
                .get_span(trait_def_id)
                .await
                .expect("a trait definition symbol should have a source span");

            if engine.get_member_by_name(symbol_id, &name).await.is_none() {
                diagnostics.receive(Diagnostic::MissingDefinition(MissingDefinition {
                    name,
                    instance_span,
                    trait_def_span,
                }));
            }
        }

        Output::new_with(Some(trait_ref), diagnostics.into_vec(), engine)
    }
}

register_build!(Key);
