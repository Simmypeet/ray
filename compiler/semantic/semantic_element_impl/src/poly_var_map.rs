use derive_more::From;
use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_handler::{Handler, Storage};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_resolution::{Obligation, discover_function_poly_vars, resolver::Resolver};
use rayc_source_file::SourceElement;
use rayc_symbol::{
    GlobalSymbolID,
    parent::get_parent_global,
    source_map::to_absolute_span,
    symbol_kind::{SymbolKind, get_symbol_kind},
    syntax::{
        get_given_parameter_list_syntax, get_parameter_list_syntax, get_type_parameter_list_syntax,
    },
};
use rayc_type::poly_var::{PolyVar, PolyVarMap, get_enclosing_poly_var_maps};

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
    DuplicatePolyVar(DuplicatePolyVar),
}

impl Report for Diagnostic {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        match self {
            Self::Resolution(diagnostic) => diagnostic.report(engine).await,
            Self::DuplicatePolyVar(diagnostic) => diagnostic.report(engine).await,
        }
    }
}

fn insert_poly_var(poly_vars: &mut PolyVarMap, poly_var: PolyVar, storage: &Storage<Diagnostic>) {
    match poly_vars.insert(poly_var) {
        Ok(_) => { /* Yay! */ }
        Err((original, id)) => {
            storage.receive(Diagnostic::DuplicatePolyVar(DuplicatePolyVar {
                name: original.name().clone(),
                original_span: poly_vars[id].span(),
                duplicate_span: original.span(),
            }));
        }
    }
}

async fn insert_given_parameters(
    engine: &TrackedEngine,
    site: GlobalSymbolID,
    poly_vars: &mut PolyVarMap,
    storage: &Storage<Diagnostic>,
    obligations: &Storage<Obligation>,
) {
    if let Some(given_parameters) = engine.get_given_parameter_list_syntax(site).await
        && let Some(given_parameters) = given_parameters.parameters()
    {
        let parent_poly_var_stack = if let Some(parent_id) = engine.get_parent_global(site).await {
            Some(engine.get_enclosing_poly_var_maps(parent_id).await)
        } else {
            None
        };

        for parameter in given_parameters.parameters() {
            let mut resolver = Resolver::builder()
                .engine(engine)
                .maybe_poly_var_stack(parent_poly_var_stack.as_deref())
                .building_poly_var_map(poly_vars)
                .site(site)
                .handler(storage)
                .obligation_handler(obligations)
                .build();

            let (Some(name), Some(trait_ref)) = (parameter.name(), parameter.trait_reference())
            else {
                continue;
            };
            let Ok(trait_ref) = resolver.resolve_trait_path(&trait_ref).await else {
                continue;
            };

            insert_poly_var(
                poly_vars,
                PolyVar::new_instance(name.kind.0.clone(), trait_ref, name.span()),
                storage,
            );
        }
    }
}

impl Build for rayc_type::poly_var::Key {
    type Diagnostic = Diagnostic;

    async fn execute(engine: &TrackedEngine, &Self { symbol_id }: &Self) -> Output<Self> {
        let storage = Storage::new();
        let obligations = Storage::new();

        let mut poly_vars = match engine.get_symbol_kind(symbol_id).await {
            SymbolKind::Def | SymbolKind::InstanceDef | SymbolKind::TraitDef => {
                let parameters = engine.get_parameter_list_syntax(symbol_id).await;
                let parent_id = engine.get_parent_global(symbol_id).await;
                let enclosing_poly_var_maps = if let Some(x) = parent_id {
                    Some(engine.get_enclosing_poly_var_maps(x).await)
                } else {
                    None
                };

                discover_function_poly_vars(parameters.as_ref(), enclosing_poly_var_maps.as_deref())
            }
            SymbolKind::Effect
            | SymbolKind::Instance
            | SymbolKind::Trait
            | SymbolKind::TraitType
            | SymbolKind::InstanceType => {
                let mut poly_vars = PolyVarMap::new();

                if let Some(type_parameters) =
                    engine.get_type_parameter_list_syntax(symbol_id).await
                {
                    for parameter in type_parameters.parameters() {
                        let Some(identifier) = parameter.name() else {
                            continue;
                        };
                        insert_poly_var(
                            &mut poly_vars,
                            PolyVar::new_type(identifier.kind.0.clone(), identifier.span()),
                            &storage,
                        );
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

        // Function-like symbols order variables by first occurrence in
        // explicit parameter types; traits and instances order explicit type
        // parameters by declaration. Given-instance variables then follow in
        // declaration order. Instance conformance relies on corresponding
        // trait and instance definitions producing the same order.
        insert_given_parameters(engine, symbol_id, &mut poly_vars, &storage, &obligations).await;

        Output::new_with(
            engine.intern(poly_vars),
            storage.into_vec(),
            obligations.into_vec(),
            engine,
        )
    }
}

register_build!(rayc_type::poly_var::Key);
