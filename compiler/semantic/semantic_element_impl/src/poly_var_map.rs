use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_handler::Dummy;
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_resolution::{discover_function_poly_vars, resolver::Resolver};
use rayc_source_file::SourceElement;
use rayc_symbol::{
    parent::get_parent_global,
    source_map::to_absolute_span,
    symbol_kind::{SymbolKind, get_symbol_kind},
    syntax::{
        get_given_parameter_list_syntax, get_parameter_list_syntax, get_type_parameter_list_syntax,
    },
};
use rayc_type::poly_var::{PolyVar, PolyVarMap, PolyVarStack, get_poly_var_map};

use crate::{
    build::{Build, Output},
    register_build,
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

fn insert_poly_var(
    poly_vars: &mut PolyVarMap,
    name: Interned<str>,
    poly_var: PolyVar,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if let Some(existing_id) = poly_vars.find_by_name(&name) {
        let original_span = poly_vars
            .iter()
            .find_map(|(id, existing)| (id == existing_id).then_some(existing.span()))
            .expect("an existing polymorphic variable ID should be valid");
        diagnostics.push(Diagnostic::DuplicatePolyVar(DuplicatePolyVar {
            name,
            original_span,
            duplicate_span: poly_var.span(),
        }));
        return;
    }

    poly_vars.insert(poly_var);
}

impl Build for rayc_type::poly_var::Key {
    type Diagnostic = Diagnostic;

    async fn execute(engine: &TrackedEngine, &Self { symbol_id }: &Self) -> Output<Self> {
        let mut diagnostics = Vec::new();
        let mut poly_vars = match engine.get_symbol_kind(symbol_id).await {
            SymbolKind::Def => {
                let parameters = engine.get_parameter_list_syntax(symbol_id).await;
                discover_function_poly_vars(parameters.as_ref())
            }
            SymbolKind::Effect | SymbolKind::Instance | SymbolKind::Trait => {
                let mut poly_vars = PolyVarMap::new();

                if let Some(type_parameters) =
                    engine.get_type_parameter_list_syntax(symbol_id).await
                {
                    for identifier in type_parameters.parameters() {
                        insert_poly_var(
                            &mut poly_vars,
                            identifier.kind.0.clone(),
                            PolyVar::new_type(identifier.kind.0.clone(), identifier.span()),
                            &mut diagnostics,
                        );
                    }
                }

                poly_vars
            }
            SymbolKind::EffectOperation => {
                panic!("an effect operation does not own a polymorphic-variable map")
            }
            SymbolKind::InstanceDef => {
                panic!("an instance def does not own a polymorphic-variable map")
            }
            SymbolKind::TraitDef => {
                panic!("a trait def does not own a polymorphic-variable map")
            }
            SymbolKind::ExternDef => {
                panic!("an extern definition does not own a polymorphic-variable map")
            }
            SymbolKind::Module => panic!("a module does not own a polymorphic-variable map"),
        };

        if let Some(given_parameters) = engine.get_given_parameter_list_syntax(symbol_id).await
            && let Some(given_parameters) = given_parameters.parameters()
        {
            let mut poly_var_stack = PolyVarStack::new();
            poly_var_stack.push(symbol_id, engine.intern(poly_vars.clone()));

            let mut parent_id = engine.get_parent_global(symbol_id).await;
            while let Some(parent) = parent_id {
                if engine.get_symbol_kind(parent).await.has_poly_var_map() {
                    poly_var_stack.push(parent, engine.get_poly_var_map(parent).await);
                }
                parent_id = engine.get_parent_global(parent).await;
            }

            let mut resolver = Resolver::new(engine, &poly_var_stack, symbol_id, &Dummy, None);

            for parameter in given_parameters.parameters() {
                let (Some(name), Some(trait_ref)) = (parameter.name(), parameter.trait_reference())
                else {
                    continue;
                };
                let Ok(trait_ref) = resolver.resolve_trait_path(&trait_ref).await else {
                    continue;
                };
                insert_poly_var(
                    &mut poly_vars,
                    name.kind.0.clone(),
                    PolyVar::new_instance(name.kind.0.clone(), trait_ref, name.span()),
                    &mut diagnostics,
                );
            }
        }

        Output::new_with(engine.intern(poly_vars), diagnostics, engine)
    }
}

register_build!(rayc_type::poly_var::Key);
