use derive_more::From;
use qbice::{Decode, Encode, Identifiable, StableHash};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_handler::{Handler, Storage};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_resolution::resolver::Resolver;
use rayc_source_file::SourceElement;
use rayc_symbol::{source_map::to_absolute_span, syntax::get_where_clause_syntax};
use rayc_syntax::where_clause::Constraint;
use rayc_type::{
    poly_var::get_enclosing_poly_var_maps,
    ty::{Ty, application::View},
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

// Check the original projection without reducing a concrete implementation.
// Resolution errors already have diagnostics and should not cause a cascade.
fn is_opaque_projection(ty: &Ty) -> Option<bool> {
    match ty {
        Ty::Application(application) => match application.view() {
            View::InstanceAssociated(associated) => {
                Some(matches!(&**associated.instance(), Ty::PolyVar(_) | Ty::SelfInstance(_)))
            }
            View::Error => None,
            View::Primitive(_)
            | View::Tuple(_)
            | View::Lambda(_)
            | View::Pointer(_)
            | View::Instance(_) => Some(false),
        },
        Ty::Inference(_) | Ty::PolyVar(_) | Ty::SelfInstance(_) | Ty::EffectRow(_) => Some(false),
    }
}

impl Build for Key {
    type Diagnostic = Diagnostic;

    async fn execute(engine: &TrackedEngine, &Self { symbol_id }: &Self) -> Output<Self> {
        let syntax = engine.get_where_clause_syntax(symbol_id).await;
        let Some(constraints) = syntax.and_then(|syntax| syntax.constraints()) else {
            return Output::new(engine.intern(WhereClause::new(engine.intern_unsized([]))), engine);
        };

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
        for constraint in constraints.constraints() {
            match constraint {
                Constraint::TypeEquality(equality) => {
                    // Incomplete constraints already have parser diagnostics.
                    let (Some(left), Some(right)) = (equality.left(), equality.right()) else {
                        continue;
                    };
                    let left_span = left.span();
                    let left = resolver.resolve_type(&left).await;
                    let right = resolver.resolve_type(&right).await;
                    match is_opaque_projection(&left) {
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
