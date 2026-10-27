use bon::Builder;
use derive_more::From;
use qbice::{Decode, Encode, Identifiable, StableHash};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_resolution::obligation::PredicateObligation;
use rayc_solver::explain::UnsatisfiedGoal;
use rayc_symbol::source_map::to_absolute_span;

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

/// An operation handler moves a value out of one of its captures.
///
/// The handlers of a `run` share their captures across every call, so no call
/// may move out of them, even if it puts a value back before returning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct MoveOutOfHandlerCapture {
    move_span: RelativeSpan,
    capture_span: RelativeSpan,
}

/// A value moved out of memory behind a reference (Rust E0507). The memory is
/// only borrowed, so it must still hold a value when the borrow ends.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Builder,
)]
pub struct MoveOutOfBorrow {
    move_span: RelativeSpan,
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Identifiable, From,
)]
pub enum Diagnostic {
    UseAfterMove(UseAfterMove),
    UseAfterPartialMove(UseAfterPartialMove),
    UseBeforeInitialization(UseBeforeInitialization),
    /// A goal required to drop a value could not be proved: no `Drop`
    /// dictionary could be selected for it, or a where-clause predicate of
    /// the selected instance does not hold.
    UnsatisfiedGoal(Box<UnsatisfiedGoal>),
    MoveOutOfHandlerCapture(MoveOutOfHandlerCapture),
    MoveOutOfBorrow(MoveOutOfBorrow),

    /// A selected `Drop` instance requires a where-clause predicate that does
    /// not hold at the dropping site, and no proof explains it.
    UnsatisfiedDropPredicate(PredicateObligation),
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
            Self::UnsatisfiedGoal(diagnostic) => diagnostic.report(engine).await,
            Self::UnsatisfiedDropPredicate(diagnostic) => diagnostic.report(engine).await,
            Self::MoveOutOfHandlerCapture(diagnostic) => {
                move_out_of_handler_capture_report(engine, diagnostic).await
            }
            Self::MoveOutOfBorrow(diagnostic) => Rendered::builder()
                .message("cannot move out of a borrow")
                .primary_highlight(
                    Highlight::builder()
                        .span(engine.to_absolute_span(&diagnostic.move_span).await)
                        .message("value moved out of a reference here")
                        .build(),
                )
                .help_message("the value is only borrowed; borrow it instead of moving it")
                .build(),
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

impl MoveOutOfHandlerCapture {
    pub(crate) const fn new(move_span: RelativeSpan, capture_span: RelativeSpan) -> Self {
        Self { move_span, capture_span }
    }
}
