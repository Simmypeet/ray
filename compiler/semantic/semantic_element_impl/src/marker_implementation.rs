use derive_more::From;
use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_handler::{Handler, Storage};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
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
    symbol_kind::{SymbolKind, get_all_symbol_ids, get_symbol_kind},
    syntax::{
        get_marker_implementation_marker_syntax, get_marker_implementation_type_syntax,
        get_where_clause_syntax, is_negative_marker_implementation,
    },
};
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
    OverlappingImplementation(OverlappingImplementation),
}

impl Report for Diagnostic {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        match self {
            Self::Resolution(diagnostic) => diagnostic.report(engine).await,
            Self::InvalidHead(diagnostic) => diagnostic.report(engine).await,
            Self::InvalidTypeVariableOccurrence(diagnostic) => diagnostic.report(engine).await,
            Self::NegativeImplementationWhereClause(diagnostic) => diagnostic.report(engine).await,
            Self::OverlappingImplementation(diagnostic) => diagnostic.report(engine).await,
        }
    }
}

fn contains_associated_type(ty: &Ty) -> bool {
    ty.recursive_iter().any(|ty| match ty {
        Ty::Application(application) => {
            matches!(application.view(), ApplicationView::InstanceAssociated(_))
        }
        Ty::Inference(_) | Ty::PolyVar(_) | Ty::SelfInstance(_) | Ty::EffectRow(_) => false,
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
        ApplicationView::Struct(struct_) => Ok(struct_.args().iter().collect()),
        ApplicationView::InstanceAssociated(_) => Err(InvalidHeadKind::AssociatedType),

        ApplicationView::Closure(_)
        | ApplicationView::DefInstance(_)
        | ApplicationView::Instance(_)
        | ApplicationView::NoOpDropInstance(_)
        | ApplicationView::TupleDropInstance(_)
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

async fn find_overlapping_implementation(
    engine: &TrackedEngine,
    symbol_id: GlobalSymbolID,
    implementation: &SemanticMarkerImplementation,
) -> Option<RelativeSpan> {
    if !implementation.has_valid_head() || implementation.marker_id().is_none() {
        return None;
    }

    let current_span = engine.get_span(symbol_id).await?;
    let current_absolute = engine.to_absolute_span(&current_span).await;
    let current_position = (current_absolute.source_id, current_absolute.start);
    let mut previous = Vec::new();
    for id in engine.get_all_symbol_ids(symbol_id.target_id).await.iter().copied() {
        let candidate = symbol_id.target_id.make_global(id);
        if engine.get_symbol_kind(candidate).await != SymbolKind::MarkerImplementation {
            continue;
        }
        let Some(span) = engine.get_span(candidate).await else {
            continue;
        };
        let absolute = engine.to_absolute_span(&span).await;
        let position = (absolute.source_id, absolute.start);

        // Only query implementations that appear before the current one. This
        // directs semantic-element dependencies from later declarations to
        // earlier ones, preventing query cycles. It also makes the later
        // declaration responsible for reporting the overlap, so the same
        // conflict is not diagnosed from both implementations.
        if position < current_position {
            previous.push((position, span, candidate));
        }
    }
    previous.sort_unstable_by_key(|(position, _, _)| *position);

    for (_, span, candidate) in previous {
        let candidate = engine.get_marker_implementation(candidate).await;
        if implementation.overlaps(&candidate) {
            return Some(span);
        }
    }

    None
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

        // Compare only against earlier declarations to keep overlap queries acyclic.
        let implementation =
            SemanticMarkerImplementation::new(marker_id, implementor, valid_head, polarity);
        if let Some(previous_span) =
            find_overlapping_implementation(engine, symbol_id, &implementation).await
        {
            diagnostics.receive(Diagnostic::OverlappingImplementation(OverlappingImplementation {
                current_span: engine
                    .get_span(symbol_id)
                    .await
                    .expect("a marker implementation should have a source span"),
                previous_span,
            }));
        }

        Output::new_with(
            engine.intern(implementation),
            diagnostics.into_vec(),
            obligations.into_vec(),
            engine,
        )
    }
}

register_build!(Key);
