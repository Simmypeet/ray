use bon::Builder;
use derive_more::From;
use qbice::{Decode, Encode, StableHash};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, From)]
pub enum Diagnostic {
    UseAfterMove(UseAfterMove),
    UseAfterPartialMove(UseAfterPartialMove),
    UseBeforeInitialization(UseBeforeInitialization),
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
