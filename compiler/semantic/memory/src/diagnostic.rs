use bon::Builder;
use derive_more::From;
use qbice::{Decode, Encode, Identifiable, StableHash};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_resolution::obligation::PredicateObligation;
use rayc_solver::instance_resolution::InstanceResolutionError;
use rayc_symbol::{name::get_qualified_name, source_map::to_absolute_span};
use rayc_type::trait_ref::TraitRef;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Builder)]
pub struct UseAfterMove {
    use_span: RelativeSpan,
    move_spans: Vec<RelativeSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Builder)]
pub struct UseAfterPartialMove {
    use_span: RelativeSpan,
    move_spans: Vec<RelativeSpan>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Builder,
)]
pub struct UseBeforeInitialization {
    use_span: RelativeSpan,
}

/// No `Drop` dictionary could be selected for a value dropped when its scope
/// ends.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct UnresolvedScopeDrop {
    /// The declaration of the binding whose value is dropped.
    binding_span: RelativeSpan,
    trait_ref: TraitRef,
    error: InstanceResolutionError,
}

/// An operation handler moves a value out of one of its captures.
///
/// The handlers of a `run` share their captures across every call, so no call
/// may move out of them, even if it puts a value back before returning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct MoveOutOfHandlerCapture {
    move_span: RelativeSpan,
    capture_span: RelativeSpan,
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Identifiable, From,
)]
pub enum Diagnostic {
    UseAfterMove(UseAfterMove),
    UseAfterPartialMove(UseAfterPartialMove),
    UseBeforeInitialization(UseBeforeInitialization),
    UnresolvedScopeDrop(UnresolvedScopeDrop),
    MoveOutOfHandlerCapture(MoveOutOfHandlerCapture),

    /// A selected `Drop` instance requires a where-clause predicate that does
    /// not hold at the dropping site.
    UnsatisfiedScopeDropPredicate(PredicateObligation),
}

async fn moved_value_report(
    engine: &TrackedEngine,
    use_span: &RelativeSpan,
    move_spans: &[RelativeSpan],
    partial: bool,
) -> Rendered<ByteIndex> {
    let related = futures::future::join_all(move_spans.iter().map(|span| async move {
        Highlight::builder()
            .span(engine.to_absolute_span(span).await)
            .message(if partial { "a component was moved here" } else { "value moved here" })
            .build()
    }))
    .await;

    Rendered::builder()
        .message(if partial { "use of partially moved value" } else { "use of moved value" })
        .primary_highlight(
            Highlight::builder()
                .span(engine.to_absolute_span(use_span).await)
                .message(if partial {
                    "value used here after one of its components was moved"
                } else {
                    "value used here after it was moved"
                })
                .build(),
        )
        .related(related)
        .build()
}

async fn unresolved_scope_drop_report(
    engine: &TrackedEngine,
    diagnostic: &UnresolvedScopeDrop,
) -> Rendered<ByteIndex> {
    // Match the wording used for implicit instances selected during type
    // checking, so the same failure reads the same at every site.
    let message = match &diagnostic.error {
        InstanceResolutionError::NotReady(_) => "cannot infer instance requirement",
        InstanceResolutionError::ContainsError(_) => "instance requirement contains an error",
        InstanceResolutionError::NoInstance { .. } => "no implicit instance found",
        InstanceResolutionError::AmbiguousLexical { .. } => "ambiguous lexical instances",
        InstanceResolutionError::AmbiguousGlobal { .. } => "ambiguous global instances",
        InstanceResolutionError::Cycle(_) => "cyclic instance resolution",
        InstanceResolutionError::Limit { .. } => "instance resolution limit exceeded",
    };

    let name = engine.get_qualified_name(diagnostic.trait_ref.trait_id()).await;
    let mut args = Vec::new();
    for arg in diagnostic.trait_ref.args().iter() {
        args.push(arg.display(engine).await.to_string());
    }

    Rendered::builder()
        .message(format!("{message}: `{name}[{}]`", args.join(", ")))
        .primary_highlight(
            Highlight::builder()
                .span(engine.to_absolute_span(&diagnostic.binding_span).await)
                .message("this value is dropped when it goes out of scope")
                .build(),
        )
        .build()
}

async fn move_out_of_handler_capture_report(
    engine: &TrackedEngine,
    diagnostic: &MoveOutOfHandlerCapture,
) -> Rendered<ByteIndex> {
    Rendered::builder()
        .message("cannot move out of a value captured by an operation handler")
        .primary_highlight(
            Highlight::builder()
                .span(engine.to_absolute_span(&diagnostic.move_span).await)
                .message("value moved out here")
                .build(),
        )
        .related(vec![
            Highlight::builder()
                .span(engine.to_absolute_span(&diagnostic.capture_span).await)
                .message("every call to the handler shares this captured value")
                .build(),
        ])
        .build()
}

impl Report for Diagnostic {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        match self {
            Self::UseAfterMove(diagnostic) => {
                moved_value_report(engine, &diagnostic.use_span, &diagnostic.move_spans, false)
                    .await
            }
            Self::UseAfterPartialMove(diagnostic) => {
                moved_value_report(engine, &diagnostic.use_span, &diagnostic.move_spans, true).await
            }
            Self::UseBeforeInitialization(diagnostic) => Rendered::builder()
                .message("use of value before initialization")
                .primary_highlight(
                    Highlight::builder()
                        .span(engine.to_absolute_span(&diagnostic.use_span).await)
                        .message("value may be uninitialized here")
                        .build(),
                )
                .build(),
            Self::UnresolvedScopeDrop(diagnostic) => {
                unresolved_scope_drop_report(engine, diagnostic).await
            }
            Self::UnsatisfiedScopeDropPredicate(diagnostic) => diagnostic.report(engine).await,
            Self::MoveOutOfHandlerCapture(diagnostic) => {
                move_out_of_handler_capture_report(engine, diagnostic).await
            }
        }
    }
}

impl UseAfterMove {
    pub(crate) const fn new(use_span: RelativeSpan, move_spans: Vec<RelativeSpan>) -> Self {
        Self { use_span, move_spans }
    }
}

impl UseAfterPartialMove {
    pub(crate) const fn new(use_span: RelativeSpan, move_spans: Vec<RelativeSpan>) -> Self {
        Self { use_span, move_spans }
    }
}

impl UseBeforeInitialization {
    pub(crate) const fn new(use_span: RelativeSpan) -> Self { Self { use_span } }
}

impl UnresolvedScopeDrop {
    pub(crate) const fn new(
        binding_span: RelativeSpan,
        trait_ref: TraitRef,
        error: InstanceResolutionError,
    ) -> Self {
        Self { binding_span, trait_ref, error }
    }
}

impl MoveOutOfHandlerCapture {
    pub(crate) const fn new(move_span: RelativeSpan, capture_span: RelativeSpan) -> Self {
        Self { move_span, capture_span }
    }
}
