use derive_more::From;
use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_handler::{Handler, Storage};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_resolution::{
    Obligation,
    discovery::{GivenTraits, discover_elided_lifetimes, discover_parameter_poly_vars},
    lifetime::lifetime_parameter_name,
    resolver::Resolver,
};
use rayc_semantic_element::callable_parameter::{CallableParameter, get_callable_parameters};
use rayc_source_file::SourceElement;
use rayc_symbol::{
    GlobalSymbolID,
    core_item::{CoreItem, get_core_item},
    parent::get_parent_global,
    source_map::to_absolute_span,
    symbol_kind::{SymbolKind, get_symbol_kind},
    syntax::{
        get_given_parameter_list_syntax, get_parameter_list_syntax, get_type_parameter_list_syntax,
    },
};
use rayc_syntax::{
    effect::{TypeParameterKind, VarianceMarker},
    r#type::Lifetime as LifetimeSyntax,
};
use rayc_type::{
    poly_var::{
        GlobalPolyVarID, PolyVar, PolyVarMap, PolyVarOrigin, PolyVarStack,
        get_enclosing_poly_var_maps,
    },
    trait_ref::TraitRef,
    ty::{Ty, args::Args},
    variance::Variance,
};

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
    ReservedLifetimeName(ReservedLifetimeName),
    MisplacedVarianceMarker(MisplacedVarianceMarker),
}

/// A lifetime parameter declared as `'static` or `'_`.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    StableHash,
    Encode,
    Decode,
    Identifiable,
)]
pub struct ReservedLifetimeName {
    span: RelativeSpan,
}

impl Report for ReservedLifetimeName {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .message("invalid lifetime parameter name")
            .primary_highlight(
                Highlight::builder()
                    .span(engine.to_absolute_span(&self.span).await)
                    .message("`'static` and `'_` cannot be declared as lifetime parameters")
                    .build(),
            )
            .build()
    }
}

/// A variance written on a parameter of a declaration other than a struct or
/// an `eff`, whose parameters have no variance.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    StableHash,
    Encode,
    Decode,
    Identifiable,
)]
pub struct MisplacedVarianceMarker {
    span: RelativeSpan,
}

impl Report for MisplacedVarianceMarker {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .message("variance cannot be declared here")
            .primary_highlight(
                Highlight::builder()
                    .span(engine.to_absolute_span(&self.span).await)
                    .message("only the parameters of a `struct` or an `eff` have a variance")
                    .build(),
            )
            .build()
    }
}

impl Report for Diagnostic {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        match self {
            Self::Resolution(diagnostic) => diagnostic.report(engine).await,
            Self::DuplicatePolyVar(diagnostic) => diagnostic.report(engine).await,
            Self::ReservedLifetimeName(diagnostic) => diagnostic.report(engine).await,
            Self::MisplacedVarianceMarker(diagnostic) => diagnostic.report(engine).await,
        }
    }
}

/// Returns the lifetime parameter `lifetime` declares, or `None` after
/// reporting a lifetime that cannot be declared.
fn lifetime_parameter(
    engine: &TrackedEngine,
    lifetime: &LifetimeSyntax,
    storage: &Storage<Diagnostic>,
) -> Option<PolyVar> {
    let identifier = lifetime.identifier();
    let identifier = match identifier {
        Some(identifier) if !lifetime.is_placeholder() => identifier,

        // `'static` and `'_` are not names.
        Some(_) | None => {
            if lifetime.name().is_some() {
                storage.receive(Diagnostic::ReservedLifetimeName(ReservedLifetimeName {
                    span: lifetime.span(),
                }));
            }
            return None;
        }
    };

    Some(PolyVar::new_lifetime(
        engine.intern_unsized(lifetime_parameter_name(&identifier.kind.0)),
        lifetime.span(),
    ))
}

/// Declares `poly_var`, reporting a name that is already declared by this
/// symbol or by an enclosing one: polymorphic variables cannot shadow.
fn insert_poly_var(
    poly_vars: &mut PolyVarMap,
    enclosing: Option<&PolyVarStack>,
    poly_var: PolyVar,
    storage: &Storage<Diagnostic>,
) {
    // The shadowing variable is still declared, so the symbol keeps its
    // arity and uses of the name inside it resolve without further errors.
    if let Some(original_span) =
        enclosing.and_then(|stack| stack.span_of(stack.find_by_name(poly_var.name())?))
    {
        storage.receive(Diagnostic::DuplicatePolyVar(DuplicatePolyVar {
            name: poly_var.name().clone(),
            original_span,
            duplicate_span: poly_var.span(),
        }));
    }

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

/// Inserts the hidden dictionaries of each callable-sugar parameter's fresh
/// type, after every source dictionary.
async fn insert_callable_dictionaries(
    engine: &TrackedEngine,
    symbol_id: GlobalSymbolID,
    callables: &[CallableParameter],
    poly_vars: &mut PolyVarMap,
) {
    for entry in callables {
        let id =
            poly_vars.find_generated(&PolyVarOrigin::CallableType(entry.occurrence())).unwrap();
        let ty = Ty::new_poly_var(GlobalPolyVarID::new(symbol_id, id), engine);

        // The callable is invoked through `Def`, and dropped through `Drop`
        // on paths that do not consume it. The fresh type is unnamed, so
        // the user cannot declare either dictionary.
        for (role, name, origin) in [
            (
                CoreItem::DefTrait,
                "callable dictionary",
                PolyVarOrigin::CallableDictionary(entry.occurrence()),
            ),
            (
                CoreItem::DropTrait,
                "callable drop dictionary",
                PolyVarOrigin::CallableDropDictionary(entry.occurrence()),
            ),
        ] {
            let requirement =
                TraitRef::new(engine.get_core_item(role).await, Args::new([ty.clone()], engine));
            poly_vars.insert_generated(
                PolyVar::new_instance(
                    engine.intern_unsized(name),
                    requirement,
                    entry.syntax().span(),
                ),
                origin,
            );
        }
    }
}

async fn insert_given_parameters(
    engine: &TrackedEngine,
    site: GlobalSymbolID,
    enclosing: Option<&PolyVarStack>,
    poly_vars: &mut PolyVarMap,
    storage: &Storage<Diagnostic>,
    obligations: &Storage<Obligation>,
) {
    if let Some(given_parameters) = engine.get_given_parameter_list_syntax(site).await
        && let Some(given_parameters) = given_parameters.parameters()
    {
        for parameter in given_parameters.parameters() {
            let mut resolver = Resolver::builder()
                .engine(engine)
                .maybe_poly_var_stack(enclosing)
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
                enclosing,
                PolyVar::new_instance(name.kind.0.clone(), trait_ref, name.span()),
                storage,
            );
        }
    }
}

/// Returns the variance a marker such as `+` declares.
const fn marked_variance(marker: &VarianceMarker) -> Variance {
    match marker {
        VarianceMarker::Covariant(_) => Variance::Covariant,
        VarianceMarker::Contravariant(_) => Variance::Contravariant,
        VarianceMarker::Invariant(_) => Variance::Invariant,
    }
}

/// Declares the explicit type and lifetime parameters of `symbol_id`.
/// Variance markers are only allowed when `has_variance` is set, and are
/// reported otherwise.
async fn declare_type_parameters(
    engine: &TrackedEngine,
    symbol_id: GlobalSymbolID,
    enclosing: Option<&PolyVarStack>,
    has_variance: bool,
    storage: &Storage<Diagnostic>,
) -> PolyVarMap {
    let mut poly_vars = PolyVarMap::new();

    if let Some(type_parameters) = engine.get_type_parameter_list_syntax(symbol_id).await {
        for parameter in type_parameters.parameters() {
            // Declare the parameter itself.
            let variable = match parameter.kind() {
                Some(TypeParameterKind::Lifetime(lifetime)) => {
                    lifetime_parameter(engine, &lifetime, storage)
                }
                Some(TypeParameterKind::Variable(parameter)) => {
                    parameter.name().map(|identifier| {
                        match crate::associated_type_kind::resolve_kind(parameter.kind_ascription())
                        {
                            rayc_type::ty::TyKind::Star => {
                                PolyVar::new_type(identifier.kind.0.clone(), identifier.span())
                            }
                            rayc_type::ty::TyKind::EffectRow => {
                                PolyVar::new_effect(identifier.kind.0.clone(), identifier.span())
                            }
                            rayc_type::ty::TyKind::Instance | rayc_type::ty::TyKind::Lifetime => {
                                unreachable!(
                                    "kind ascriptions cannot declare dictionaries or lifetimes"
                                )
                            }
                        }
                    })
                }
                None => None,
            };
            let Some(mut variable) = variable else {
                continue;
            };

            // Attach its declared variance, where variances exist.
            if let Some(marker) = parameter.variance() {
                if has_variance {
                    variable = variable.with_declared_variance(marked_variance(&marker));
                } else {
                    storage.receive(Diagnostic::MisplacedVarianceMarker(MisplacedVarianceMarker {
                        span: marker.span(),
                    }));
                }
            }

            insert_poly_var(&mut poly_vars, enclosing, variable, storage);
        }
    }

    poly_vars
}

impl Build for rayc_type::poly_var::Key {
    type Diagnostic = Diagnostic;

    async fn execute(engine: &TrackedEngine, &Self { symbol_id }: &Self) -> Output<Self> {
        let storage = Storage::new();
        let obligations = Storage::new();

        let symbol_kind = engine.get_symbol_kind(symbol_id).await;

        // The variables of every enclosing symbol, which this symbol's
        // declarations must not shadow.
        let enclosing = match engine.get_parent_global(symbol_id).await {
            Some(parent_id) => Some(engine.get_enclosing_poly_var_maps(parent_id).await),
            None => None,
        };

        let mut poly_vars = match symbol_kind {
            SymbolKind::Def | SymbolKind::InstanceDef | SymbolKind::TraitDef => {
                let parameters = engine.get_parameter_list_syntax(symbol_id).await;

                // The given parameters are resolved after the type parameters,
                // but their traits are known to discovery; see `GivenTraits`.
                let given_parameters = engine.get_given_parameter_list_syntax(symbol_id).await;
                let given_traits =
                    GivenTraits::new(engine, symbol_id, given_parameters.as_ref()).await;

                // Only a plain `def` introduces lifetimes for elision. Trait
                // and instance defs forbid elided lifetimes in their
                // parameter types.
                let introduce_elided = symbol_kind == SymbolKind::Def;

                // An explicit type-parameter list declares every named
                // variable, so only elided lifetimes are discovered after it.
                // Without one, parameter types introduce their variables.
                if engine.get_type_parameter_list_syntax(symbol_id).await.is_some() {
                    let poly_vars = declare_type_parameters(
                        engine,
                        symbol_id,
                        enclosing.as_deref(),
                        symbol_kind.has_variance_map(),
                        &storage,
                    )
                    .await;

                    if introduce_elided {
                        discover_elided_lifetimes(
                            engine,
                            symbol_id,
                            parameters.as_ref(),
                            &given_traits,
                            enclosing.as_deref(),
                            poly_vars,
                        )
                        .await
                    } else {
                        poly_vars
                    }
                } else {
                    discover_parameter_poly_vars(
                        engine,
                        symbol_id,
                        parameters.as_ref(),
                        &given_traits,
                        enclosing.as_deref(),
                        introduce_elided,
                    )
                    .await
                }
            }
            SymbolKind::Effect
            | SymbolKind::Instance
            | SymbolKind::MarkerImplementation
            | SymbolKind::Strut
            | SymbolKind::Trait
            | SymbolKind::TraitType
            | SymbolKind::InstanceType => {
                declare_type_parameters(
                    engine,
                    symbol_id,
                    enclosing.as_deref(),
                    symbol_kind.has_variance_map(),
                    &storage,
                )
                .await
            }
            SymbolKind::EffectOperation => {
                panic!("an effect operation does not own a polymorphic-variable map")
            }
            SymbolKind::ExternDef => {
                panic!("an extern definition does not own a polymorphic-variable map")
            }
            SymbolKind::Marker => panic!("a marker does not own a polymorphic-variable map"),
            SymbolKind::Module => panic!("a module does not own a polymorphic-variable map"),
        };

        // Symbols with an explicit type-parameter list order its variables by
        // declaration; other function-like symbols order them by first
        // occurrence in parameter types. Generated types follow source types, then
        // explicit dictionaries precede generated dictionaries. Source given
        // arguments therefore retain their original positional order.
        let callables = engine.get_callable_parameters(symbol_id).await;
        for entry in callables.iter() {
            poly_vars.insert_generated(
                PolyVar::new_type(engine.intern_unsized("callable"), entry.syntax().span()),
                PolyVarOrigin::CallableType(entry.occurrence()),
            );
        }

        insert_given_parameters(
            engine,
            symbol_id,
            enclosing.as_deref(),
            &mut poly_vars,
            &storage,
            &obligations,
        )
        .await;
        insert_callable_dictionaries(engine, symbol_id, &callables, &mut poly_vars).await;

        Output::new_with(
            engine.intern(poly_vars),
            storage.into_vec(),
            obligations.into_vec(),
            engine,
        )
    }
}

register_build!(rayc_type::poly_var::Key);
