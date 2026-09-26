use derive_more::From;
use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Identifiable, StableHash, executor, program::Registration,
    storage::intern::Interned,
};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_handler::{Handler, Storage};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_resolution::{Obligation, lifetime::LifetimeElision, resolver::Resolver};
use rayc_semantic_element::callable_parameter::get_callable_parameters;
use rayc_solver::outlives::implied::implied_bounds;
use rayc_source_file::SourceElement;
use rayc_symbol::{
    GlobalSymbolID,
    core_item::{CoreItem, get_core_item},
    source_map::to_absolute_span,
    span::get_span,
    symbol_kind::{SymbolKind, get_symbol_kind},
    syntax::get_where_clause_syntax,
};
use rayc_syntax::where_clause::{Constraint, OutlivesPredicate, TypeEquality};
use rayc_type::{
    poly_var::{GlobalPolyVarID, PolyVarOrigin, get_enclosing_poly_var_maps, get_poly_var_map},
    ty::{Ty, TyKind},
    where_clause::{
        AssociatedTypeEquality, DeclaredKey, Key, MarkerPredicate, Predicate, PredicateKind,
        PredicateOrigin, WhereClause, get_declared_where_clause,
    },
};

use crate::{
    build::{Build, ObligationKey, Output},
    register_build,
};

/// The left operand must project from an abstract dictionary, not an
/// implementation.
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
pub struct InvalidEqualityLeft {
    span: RelativeSpan,
}

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
enum InvalidMarkerImplementationPredicateKind {
    NonMarkerPredicate,
    NonVariableImplementor,
}

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
pub struct InvalidMarkerImplementationPredicate {
    span: RelativeSpan,
    kind: InvalidMarkerImplementationPredicateKind,
}

impl Report for InvalidMarkerImplementationPredicate {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let message = match self.kind {
            InvalidMarkerImplementationPredicateKind::NonMarkerPredicate => {
                "marker implementation where clauses may only contain marker predicates"
            }
            InvalidMarkerImplementationPredicateKind::NonVariableImplementor => {
                "a marker predicate in an implementation where clause must apply directly to a \
                 polymorphic type variable"
            }
        };

        Rendered::builder()
            .message("invalid marker implementation where-clause predicate")
            .primary_highlight(Highlight::new(
                engine.to_absolute_span(&self.span).await,
                Some(message.to_owned()),
            ))
            .build()
    }
}

impl Report for InvalidEqualityLeft {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .message(
                "the left-hand side of a where equality must be an opaque instance associated type",
            )
            .primary_highlight(
                Highlight::builder()
                    .span(engine.to_absolute_span(&self.span).await)
                    .message("expected a projection through a given dictionary or trait `this`")
                    .build(),
            )
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
    InvalidEqualityLeft(InvalidEqualityLeft),
    InvalidMarkerImplementationPredicate(InvalidMarkerImplementationPredicate),
}

impl Report for Diagnostic {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        match self {
            Self::Resolution(diagnostic) => diagnostic.report(engine).await,
            Self::InvalidEqualityLeft(diagnostic) => diagnostic.report(engine).await,
            Self::InvalidMarkerImplementationPredicate(diagnostic) => {
                diagnostic.report(engine).await
            }
        }
    }
}

impl Build for DeclaredKey {
    type Diagnostic = Diagnostic;

    async fn execute(engine: &TrackedEngine, &Self { symbol_id }: &Self) -> Output<Self> {
        let syntax = engine.get_where_clause_syntax(symbol_id).await;
        let constraints = syntax.and_then(|syntax| syntax.constraints());
        let is_marker_implementation =
            engine.get_symbol_kind(symbol_id).await == SymbolKind::MarkerImplementation;

        // Resolve both operands in the declaration's polymorphic and given scope.
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

        let mut predicates = Vec::new();
        for constraint in
            constraints.iter().flat_map(rayc_syntax::where_clause::Constraints::constraints)
        {
            match constraint {
                Constraint::TypeEquality(equality) => predicates.extend(
                    resolve_type_equality(
                        engine,
                        &mut resolver,
                        &diagnostics,
                        &equality,
                        is_marker_implementation,
                    )
                    .await,
                ),
                // Marker implementations assume the requirements of their
                // head (see `head_requirements`), and marker entailment ignores
                // lifetimes, so an outlives predicate there has no use.
                Constraint::OutlivesPredicate(syntax) if is_marker_implementation => {
                    diagnostics.receive(Diagnostic::InvalidMarkerImplementationPredicate(
                        InvalidMarkerImplementationPredicate {
                            span: syntax.span(),
                            kind: InvalidMarkerImplementationPredicateKind::NonMarkerPredicate,
                        },
                    ));
                }
                // Only `def`s, structs, and marker implementations have implied
                // bounds, so other declarations must write theirs.
                Constraint::OutlivesPredicate(syntax) => {
                    if let Some(predicate) =
                        resolve_outlives_predicate(&mut resolver, &syntax).await
                    {
                        predicates.push(Predicate::new(
                            PredicateKind::Outlives(predicate),
                            syntax.span(),
                            PredicateOrigin::Declared,
                        ));
                    }
                }
                Constraint::MarkerPredicate(predicate) => {
                    // Incomplete predicates already have parser diagnostics.
                    let (Some(implementor_syntax), Some(marker_syntax)) =
                        (predicate.implementor(), predicate.marker())
                    else {
                        continue;
                    };

                    let implementor = resolver.resolve_type(&implementor_syntax).await;
                    let marker_id = resolver.resolve_marker_path(&marker_syntax).await.ok();

                    // Marker implementation contexts deliberately use only
                    // variable-headed predicates, keeping their assumptions in the
                    // same simple form as their implementation heads.
                    if is_marker_implementation
                        && !implementor.contains_error()
                        && implementor.as_poly_var().is_none()
                    {
                        diagnostics.receive(Diagnostic::InvalidMarkerImplementationPredicate(
                            InvalidMarkerImplementationPredicate {
                                span: implementor_syntax.span(),
                                kind: InvalidMarkerImplementationPredicateKind::NonVariableImplementor,
                            },
                        ));
                        continue;
                    }

                    if let Some(marker_id) = marker_id
                        && !implementor.contains_error()
                    {
                        predicates.push(Predicate::new(
                            PredicateKind::Marker(MarkerPredicate::new(marker_id, implementor)),
                            predicate.span(),
                            PredicateOrigin::Declared,
                        ));
                    }
                }
            }
        }

        // Elaborate each callable contract in the completed declaration scope.
        for entry in engine.get_callable_parameters(symbol_id).await.iter() {
            elaborate_callable(engine, &mut resolver, entry, &mut predicates).await;
        }

        // Preserve the predicates as assumptions for later semantic consumers.
        // Implied bounds are added by the complete where clause query.
        Output::new_with(
            engine.intern(WhereClause::new(engine.intern_unsized(predicates))),
            diagnostics.into_vec(),
            obligations.into_vec(),
            engine,
        )
    }
}

register_build!(DeclaredKey);

/// Completes the declared where clause of a symbol with its implied bounds.
#[executor(config = Config)]
async fn where_clause_executor(
    &Key { symbol_id }: &Key,
    engine: &TrackedEngine,
) -> Interned<WhereClause> {
    // Only symbols that support a where clause have declared predicates.
    let mut predicates = Vec::new();
    if engine.get_symbol_kind(symbol_id).await.has_where_clause() {
        let declared = engine.get_declared_where_clause(symbol_id).await;
        predicates.extend(declared.predicates().cloned());
    }

    // Implied bounds point at the declaration that implies them.
    let implied = implied_bounds(symbol_id, engine).await;
    if !implied.is_empty() {
        let span =
            engine.get_span(symbol_id).await.expect("a declaration with implied bounds has a span");
        predicates.extend(implied.into_iter().map(|predicate| {
            Predicate::new(PredicateKind::Outlives(predicate), span, PredicateOrigin::Implied)
        }));
    }

    if engine.get_symbol_kind(symbol_id).await == SymbolKind::MarkerImplementation {
        predicates.extend(head_requirements(symbol_id, engine).await);
    }

    engine.intern(WhereClause::new(engine.intern_unsized(predicates)))
}

#[distributed_slice(RAY_PROGRAM)]
static WHERE_CLAUSE_EXECUTOR: Registration<Config> =
    Registration::new::<Key, WhereClauseExecutor>();

/// Returns the requirements for naming the head of a marker implementation,
/// which the implementation assumes instead of spelling out.
///
/// An implementation only applies to a goal type that matches its head, and
/// that goal type is well-formed where it is named, so these requirements
/// hold wherever the implementation is used. A valid head is one constructor
/// applied to distinct variables, so they are the requirements of that
/// constructor stated over those variables, such as `t: 'a` for `&'a t` or
/// the where clause of the struct `S[t]`.
async fn head_requirements(symbol_id: GlobalSymbolID, engine: &TrackedEngine) -> Vec<Predicate> {
    let key = rayc_semantic_element::marker_implementation::Key { symbol_id };
    let obligations = engine.query(&ObligationKey::new(key)).await;

    let mut predicates = Vec::new();
    for obligation in obligations.iter() {
        match obligation {
            Obligation::WfCheck(check) => {
                for obligation in check.predicate_obligations(engine).await {
                    predicates.push(Predicate::new(
                        obligation.predicate().clone(),
                        obligation.span(),
                        PredicateOrigin::Implied,
                    ));
                }
            }
            Obligation::ReferenceWf(check) => predicates.push(Predicate::new(
                PredicateKind::Outlives(check.predicate()),
                check.span(),
                PredicateOrigin::Implied,
            )),
            // A head has no given arguments to check.
            Obligation::TraitRefCheck(_) => {}
        }
    }
    predicates
}

/// Resolves a type equality of a where clause into a predicate, reporting an
/// equality that is not allowed.
async fn resolve_type_equality(
    engine: &TrackedEngine,
    resolver: &mut Resolver<'_>,
    diagnostics: &dyn Handler<Diagnostic>,
    equality: &TypeEquality,
    is_marker_implementation: bool,
) -> Option<Predicate> {
    // Marker implementations only accept marker predicates.
    if is_marker_implementation {
        diagnostics.receive(Diagnostic::InvalidMarkerImplementationPredicate(
            InvalidMarkerImplementationPredicate {
                span: equality.equals().map_or_else(|| equality.span(), |equals| equals.span()),
                kind: InvalidMarkerImplementationPredicateKind::NonMarkerPredicate,
            },
        ));
        return None;
    }

    // Incomplete constraints already have parser diagnostics.
    let (Some(left), Some(right)) = (equality.left(), equality.right()) else {
        return None;
    };
    let left_span = left.span();

    // Infer the left operand's kind, then check the right against it.
    let left = resolver.infer_type_term(&left).await;
    let right = resolver.resolve_type_term(&right, left.kind_of(engine).await).await;

    match left.is_opaque_projection() {
        Some(true) => {}
        Some(false) => {
            diagnostics
                .receive(Diagnostic::InvalidEqualityLeft(InvalidEqualityLeft { span: left_span }));
            return None;
        }
        None => return None,
    }

    Some(Predicate::new(
        PredicateKind::AssociatedTypeEquality(AssociatedTypeEquality::new(left, right)),
        equality.span(),
        PredicateOrigin::Declared,
    ))
}

/// Resolves an outlives predicate, such as `'a: 'b` or `t: 'a`. The subject
/// may be of any kind, a dictionary included.
///
/// Returns `None` when an operand is missing or failed to resolve, since
/// those are already reported.
async fn resolve_outlives_predicate(
    resolver: &mut Resolver<'_>,
    syntax: &OutlivesPredicate,
) -> Option<rayc_type::where_clause::OutlivesPredicate> {
    let (Some(bounded_syntax), Some(bound_syntax)) = (syntax.bounded(), syntax.bound()) else {
        return None;
    };

    let bounded = resolver.infer_type_term(&bounded_syntax).await;
    let bound = resolver.resolve_lifetime(&bound_syntax).await;
    if bounded.contains_error() || bound.contains_error() {
        return None;
    }

    Some(rayc_type::where_clause::OutlivesPredicate::new(bounded, bound))
}

async fn elaborate_callable(
    engine: &TrackedEngine,
    resolver: &mut Resolver<'_>,
    entry: &rayc_semantic_element::callable_parameter::CallableParameter,
    predicates: &mut Vec<Predicate>,
) {
    let syntax = entry.syntax();
    let map = engine.get_poly_var_map(entry.owner()).await;
    let id = map.find_generated(&PolyVarOrigin::CallableDictionary(entry.occurrence())).unwrap();
    let dictionary = Ty::new_poly_var(GlobalPolyVarID::new(entry.owner(), id), engine);

    // The callable's own signature cannot elide lifetimes.
    let elision = resolver.replace_lifetime_elision(LifetimeElision::HigherRanked);

    let mut args = Vec::new();
    if let Some(parameters) = syntax.parameters() {
        for parameter in parameters.parameters() {
            args.push(resolver.resolve_type(&parameter).await);
        }
    }

    // Constructs the callable's args type as a tuple type
    let args = Ty::new_tuple(engine.intern_unsized(args), engine);

    // Return type of the callable
    let result = if let Some(annotation) = syntax.return_type() {
        if let Some(ty) = annotation.r#type() {
            resolver.resolve_type(&ty).await
        } else {
            Ty::new_star_error(engine)
        }
    } else {
        // If no return type is specified, the callable returns the unit type.
        Ty::new_tuple(engine.intern_unsized([]), engine)
    };

    // Effect row of the callable
    let effect = if let Some(annotation) = syntax.effect_row() {
        if let Some(row) = annotation.effect_row() {
            resolver.resolve_effect_row(&row).await
        } else {
            Ty::new_error(TyKind::EffectRow, engine)
        }
    } else {
        // If no effect row is specified, the callable has an empty effect row.
        Ty::new_effect_row([], None, engine)
    };
    resolver.replace_lifetime_elision(elision);

    for (role, right, span) in [
        (
            CoreItem::DefArgs,
            args,
            syntax.parameters().map_or_else(|| syntax.span(), |parameters| parameters.span()),
        ),
        (
            CoreItem::DefReturn,
            result,
            syntax.return_type().map_or_else(|| syntax.span(), |annotation| annotation.span()),
        ),
        (
            CoreItem::DefEffect,
            effect,
            syntax.effect_row().map_or_else(|| syntax.span(), |annotation| annotation.span()),
        ),
    ] {
        let left = Ty::new_instance_associated(
            engine.get_core_item(role).await,
            dictionary.clone(),
            [],
            engine,
        );
        predicates.push(Predicate::new(
            PredicateKind::AssociatedTypeEquality(AssociatedTypeEquality::new(left, right)),
            span,
            PredicateOrigin::Declared,
        ));
    }
}
