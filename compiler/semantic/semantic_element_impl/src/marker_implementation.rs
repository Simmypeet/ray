use derive_more::From;
use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Identifiable, Query, StableHash, executor, program::Registration,
    storage::intern::Interned,
};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_handler::{Handler, Storage};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_resolution::resolver::Resolver;
use rayc_semantic_element::marker_implementation::{
    Key, MarkerImplementation as SemanticMarkerImplementation, MarkerImplementationPolarity,
    get_marker_implementation,
};
use rayc_source_file::SourceElement;
use rayc_symbol::{
    GlobalSymbolID,
    source_map::to_absolute_span,
    span::get_span,
    symbol_kind::get_all_marker_implementation_ids,
    syntax::{
        get_marker_implementation_marker_syntax, get_marker_implementation_type_syntax,
        get_where_clause_syntax, is_negative_marker_implementation,
    },
};
use rayc_target::TargetID;
use rayc_type::{
    poly_var::{GlobalPolyVarID, get_enclosing_poly_var_maps, get_poly_var_map},
    ty::{Ty, application::View as ApplicationView},
};

use crate::{
    build::{Build, Output},
    register_build,
};

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
enum InvalidHeadKind {
    AssociatedType,
    MissingTypeConstructor,
    NonVariableArgument,
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
pub struct InvalidHead {
    span: RelativeSpan,
    kind: InvalidHeadKind,
}

impl Report for InvalidHead {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let message = match self.kind {
            InvalidHeadKind::AssociatedType => {
                "associated types are not allowed in marker implementation heads"
            }
            InvalidHeadKind::MissingTypeConstructor => {
                "a marker implementation head must start with a type constructor"
            }
            InvalidHeadKind::NonVariableArgument => {
                "type constructor arguments in a marker implementation must be type variables"
            }
        };

        Rendered::builder()
            .message("invalid marker implementation head")
            .primary_highlight(Highlight::new(
                engine.to_absolute_span(&self.span).await,
                Some(message.to_owned()),
            ))
            .build()
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct InvalidTypeVariableOccurrence {
    name: Interned<str>,
    span: RelativeSpan,
    occurrences: usize,
}

impl Report for InvalidTypeVariableOccurrence {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .message("invalid marker implementation head")
            .primary_highlight(Highlight::new(
                engine.to_absolute_span(&self.span).await,
                Some(format!(
                    "type variable `{}` must occur exactly once in the implementation head; found \
                     {} occurrences",
                    &*self.name, self.occurrences
                )),
            ))
            .build()
    }
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
pub struct OverlappingImplementation {
    current_span: RelativeSpan,
    previous_span: RelativeSpan,
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
pub struct NegativeImplementationWhereClause {
    span: RelativeSpan,
}

impl Report for NegativeImplementationWhereClause {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .message("negative marker implementations cannot have where clauses")
            .primary_highlight(Highlight::new(
                engine.to_absolute_span(&self.span).await,
                Some("remove this conditional requirement".into()),
            ))
            .build()
    }
}

impl Report for OverlappingImplementation {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .message("overlapping marker implementations")
            .primary_highlight(Highlight::new(
                engine.to_absolute_span(&self.current_span).await,
                Some("this marker is already implemented for the same type constructor".into()),
            ))
            .related(vec![Highlight::new(
                engine.to_absolute_span(&self.previous_span).await,
                Some("the first overlapping implementation is here".into()),
            )])
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
    InvalidHead(InvalidHead),
    InvalidTypeVariableOccurrence(InvalidTypeVariableOccurrence),
    NegativeImplementationWhereClause(NegativeImplementationWhereClause),
}

impl Report for Diagnostic {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        match self {
            Self::Resolution(diagnostic) => diagnostic.report(engine).await,
            Self::InvalidHead(diagnostic) => diagnostic.report(engine).await,
            Self::InvalidTypeVariableOccurrence(diagnostic) => diagnostic.report(engine).await,
            Self::NegativeImplementationWhereClause(diagnostic) => diagnostic.report(engine).await,
        }
    }
}

fn contains_associated_type(ty: &Ty) -> bool {
    ty.recursive_iter().any(|ty| match ty {
        Ty::Application(application) => {
            matches!(application.view(), ApplicationView::InstanceAssociated(_))
        }
        Ty::Inference(_)
        | Ty::PolyVar(_)
        | Ty::SelfInstance(_)
        | Ty::EffectRow(_)
        | Ty::Lifetime(_) => false,
    })
}

fn head_arguments(ty: &Ty) -> Result<Vec<&Interned<Ty>>, InvalidHeadKind> {
    let Ty::Application(application) = ty else {
        return Err(InvalidHeadKind::MissingTypeConstructor);
    };

    match application.view() {
        ApplicationView::Primitive(_) => Ok(Vec::new()),
        ApplicationView::Tuple(tuple) => Ok(tuple.args().iter().collect()),
        ApplicationView::Pointer(pointer) => Ok(vec![pointer.pointee()]),
        ApplicationView::Reference(reference) => {
            Ok(vec![reference.lifetime(), reference.pointee()])
        }
        ApplicationView::Struct(struct_) => Ok(struct_.args().iter().collect()),
        ApplicationView::Enum(enum_) => Ok(enum_.args().iter().collect()),
        ApplicationView::InstanceAssociated(_) => Err(InvalidHeadKind::AssociatedType),

        ApplicationView::Closure(_)
        | ApplicationView::DefInstance(_)
        | ApplicationView::Instance(_)
        | ApplicationView::NoOpDropInstance(_)
        | ApplicationView::TupleDropInstance(_)
        | ApplicationView::ClosureDropInstance(_)
        | ApplicationView::DefCoroutine(_)
        | ApplicationView::TraitDefCoroutine(_)
        | ApplicationView::ClosureCoroutine(_)
        | ApplicationView::ContinuationDefInstance(_)
        | ApplicationView::CoroutineInstance(_)
        | ApplicationView::CoroutineDropInstance(_)
        | ApplicationView::GeneratedAdtDropInstance(_)
        | ApplicationView::Error => Err(InvalidHeadKind::MissingTypeConstructor),
    }
}

async fn validate_head(
    engine: &TrackedEngine,
    symbol_id: GlobalSymbolID,
    implementor: &Interned<Ty>,
    span: RelativeSpan,
    diagnostics: &Storage<Diagnostic>,
) -> bool {
    // Resolution errors already explain malformed or incorrectly-kinded types.
    if implementor.contains_error() {
        return false;
    }

    if contains_associated_type(implementor) {
        diagnostics.receive(Diagnostic::InvalidHead(InvalidHead {
            span,
            kind: InvalidHeadKind::AssociatedType,
        }));
        return false;
    }

    let arguments = match head_arguments(implementor) {
        Ok(arguments) => arguments,
        Err(kind) => {
            diagnostics.receive(Diagnostic::InvalidHead(InvalidHead { span, kind }));
            return false;
        }
    };

    if arguments.iter().copied().any(|argument| argument.as_poly_var().is_none()) {
        diagnostics.receive(Diagnostic::InvalidHead(InvalidHead {
            span,
            kind: InvalidHeadKind::NonVariableArgument,
        }));
        return false;
    }

    // Every implementation-owned type variable must occur exactly once.
    let parameters = engine.get_poly_var_map(symbol_id).await;
    let mut valid = true;
    for (id, parameter) in parameters.iter() {
        let id = GlobalPolyVarID::new(symbol_id, id);
        let occurrences = arguments
            .iter()
            .copied()
            .filter(|argument| argument.as_poly_var().copied() == Some(id))
            .count();

        if occurrences != 1 {
            diagnostics.receive(Diagnostic::InvalidTypeVariableOccurrence(
                InvalidTypeVariableOccurrence {
                    name: parameter.name().clone(),
                    span: parameter.span(),
                    occurrences,
                },
            ));
            valid = false;
        }
    }

    valid
}

/// An implementation that overlaps an earlier one, in source order, with the
/// diagnostic it reports.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
struct Overlap {
    implementation: GlobalSymbolID,
    diagnostic: OverlappingImplementation,
}

/// The overlaps between the marker implementations of a target: every
/// implementation that overlaps an earlier one in source order, with the first
/// earlier one it overlaps.
///
/// Overlap is a property of the whole set of implementations, decided in source
/// order, which reads the text of every file declaring one. Only the
/// diagnostics read it, so that an edit to such a file dirties no semantic
/// query. It is a firewall: the edit makes it recompute, but the diagnostics
/// only when an overlap changes.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<[Overlap]>)]
struct OverlapsKey {
    target_id: TargetID,
}

#[executor(config = Config, style = qbice::ExecutionStyle::Firewall)]
async fn overlaps_executor(
    &OverlapsKey { target_id }: &OverlapsKey,
    engine: &TrackedEngine,
) -> Interned<[Overlap]> {
    // the marker implementations of the target in source order
    let mut ordered = Vec::new();
    for id in engine.get_all_marker_implementation_ids(target_id).await.iter().copied() {
        let symbol_id = target_id.make_global(id);
        let Some(span) = engine.get_span(symbol_id).await else {
            continue;
        };
        let absolute = engine.to_absolute_span(&span).await;
        let implementation = engine.get_marker_implementation(symbol_id).await;
        ordered.push(((absolute.source_id, absolute.start), span, symbol_id, implementation));
    }
    ordered.sort_unstable_by_key(|(position, ..)| *position);

    // a later implementation reports the first earlier one it overlaps, so
    // that the same conflict is not reported from both
    let mut overlaps = Vec::new();
    for (index, (_, current_span, symbol_id, implementation)) in ordered.iter().enumerate() {
        if !implementation.has_valid_head() || implementation.marker_id().is_none() {
            continue;
        }

        let previous =
            ordered[..index].iter().find(|(_, _, _, candidate)| implementation.overlaps(candidate));
        if let Some((_, previous_span, ..)) = previous {
            overlaps.push(Overlap {
                implementation: *symbol_id,
                diagnostic: OverlappingImplementation {
                    current_span: *current_span,
                    previous_span: *previous_span,
                },
            });
        }
    }

    engine.intern_unsized(overlaps)
}

#[distributed_slice(RAY_PROGRAM)]
static OVERLAPS_EXECUTOR: Registration<Config> =
    Registration::new::<OverlapsKey, OverlapsExecutor>();

/// Returns the overlap the marker implementation `symbol_id` reports, if it
/// overlaps an earlier implementation of its target.
pub(crate) async fn get_overlapping_implementation(
    engine: &TrackedEngine,
    symbol_id: GlobalSymbolID,
) -> Option<OverlappingImplementation> {
    let overlaps = engine.query(&OverlapsKey { target_id: symbol_id.target_id }).await;

    overlaps
        .iter()
        .find(|overlap| overlap.implementation == symbol_id)
        .map(|overlap| overlap.diagnostic)
}

impl Build for Key {
    type Diagnostic = Diagnostic;

    async fn execute(engine: &TrackedEngine, &Self { symbol_id }: &Self) -> Output<Self> {
        let diagnostics = Storage::new();
        let obligations = Storage::new();
        let negative = engine.is_negative_marker_implementation(symbol_id).await;
        let polarity = if negative {
            MarkerImplementationPolarity::Negative
        } else {
            MarkerImplementationPolarity::Positive
        };
        let poly_vars = engine.get_enclosing_poly_var_maps(symbol_id).await;
        let mut resolver = Resolver::builder()
            .engine(engine)
            .poly_var_stack(&poly_vars)
            .site(symbol_id)
            .handler(&diagnostics)
            .obligation_handler(&obligations)
            .build();

        // Resolve the marker name without attempting marker entailment.
        let marker_id =
            if let Some(syntax) = engine.get_marker_implementation_marker_syntax(symbol_id).await {
                resolver.resolve_marker_path(&syntax).await.ok()
            } else {
                None
            };

        // Resolve and validate the simple implementation head.
        let type_syntax = engine.get_marker_implementation_type_syntax(symbol_id).await;
        let implementor = if let Some(syntax) = type_syntax.as_ref() {
            resolver.resolve_type(syntax).await
        } else {
            Ty::new_star_error(engine)
        };
        let valid_head = if let Some(syntax) = type_syntax.as_ref() {
            validate_head(engine, symbol_id, &implementor, syntax.span(), &diagnostics).await
        } else {
            false
        };

        // Conditional negative reasoning needs a three-valued applicability
        // check, which the Boolean marker solver deliberately does not expose.
        if negative && let Some(where_clause) = engine.get_where_clause_syntax(symbol_id).await {
            diagnostics.receive(Diagnostic::NegativeImplementationWhereClause(
                NegativeImplementationWhereClause { span: where_clause.span() },
            ));
        }

        // overlaps with the other implementations are checked over the whole
        // target, by `OverlapsKey`
        let implementation =
            SemanticMarkerImplementation::new(marker_id, implementor, valid_head, polarity);

        Output::new_with(
            engine.intern(implementation),
            diagnostics.into_vec(),
            obligations.into_vec(),
            engine,
        )
    }
}

register_build!(Key);
