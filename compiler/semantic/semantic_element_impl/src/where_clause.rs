use derive_more::From;
use qbice::{Decode, Encode, Identifiable, StableHash};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_handler::{Handler, Storage};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_resolution::resolver::Resolver;
use rayc_semantic_element::callable_parameter::get_callable_parameters;
use rayc_source_file::SourceElement;
use rayc_symbol::{
    core_item::{CoreItem, get_core_item},
    source_map::to_absolute_span,
    syntax::get_where_clause_syntax,
};
use rayc_syntax::where_clause::Constraint;
use rayc_type::{
    poly_var::{GlobalPolyVarID, PolyVarOrigin, get_enclosing_poly_var_maps, get_poly_var_map},
    ty::{Ty, TyKind},
    where_clause::{AssociatedTypeEquality, Key, Predicate, PredicateKind, WhereClause},
};

use crate::{
    build::{Build, Output},
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
}

impl Report for Diagnostic {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        match self {
            Self::Resolution(diagnostic) => diagnostic.report(engine).await,
            Self::InvalidEqualityLeft(diagnostic) => diagnostic.report(engine).await,
        }
    }
}

impl Build for Key {
    type Diagnostic = Diagnostic;

    async fn execute(engine: &TrackedEngine, &Self { symbol_id }: &Self) -> Output<Self> {
        let syntax = engine.get_where_clause_syntax(symbol_id).await;
        let constraints = syntax.and_then(|syntax| syntax.constraints());

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
                Constraint::TypeEquality(equality) => {
                    // Incomplete constraints already have parser diagnostics.
                    let (Some(left), Some(right)) = (equality.left(), equality.right()) else {
                        continue;
                    };
                    let left_span = left.span();
                    // Infer the left operand's kind, then check the right against it.
                    let left = resolver.infer_type_term(&left).await;
                    let right =
                        resolver.resolve_type_term(&right, left.kind_of(engine).await).await;

                    match left.is_opaque_projection() {
                        Some(true) => {}
                        Some(false) => {
                            diagnostics.receive(Diagnostic::InvalidEqualityLeft(
                                InvalidEqualityLeft { span: left_span },
                            ));
                            continue;
                        }
                        None => continue,
                    }

                    predicates.push(Predicate::new(
                        PredicateKind::AssociatedTypeEquality(AssociatedTypeEquality::new(
                            left, right,
                        )),
                        equality.span(),
                    ));
                }
            }
        }

        // Elaborate each callable contract in the completed declaration scope.
        for entry in engine.get_callable_parameters(symbol_id).await.iter() {
            elaborate_callable(engine, &mut resolver, entry, &mut predicates).await;
        }

        // Preserve the predicates as assumptions for later semantic consumers.
        Output::new_with(
            engine.intern(WhereClause::new(engine.intern_unsized(predicates))),
            diagnostics.into_vec(),
            obligations.into_vec(),
            engine,
        )
    }
}

register_build!(Key);

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
        ));
    }
}
