use bon::Builder;
use derive_more::From;
use qbice::{Decode, Encode, Identifiable, StableHash};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_symbol::source_map::to_absolute_span;

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    StableHash,
    Encode,
    Decode,
    Identifiable,
    Builder,
)]
pub struct NotAllPathsReturnValue {
    span: RelativeSpan,
}

impl Report for NotAllPathsReturnValue {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .message("not all paths return a value")
            .primary_highlight(
                Highlight::builder()
                    .span(engine.to_absolute_span(&self.span).await)
                    .message("this function can reach its end without returning a value")
                    .build(),
            )
            .build()
    }
}

/// A diagnostic found while building the IR of a definition.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Identifiable, From,
)]
pub enum Diagnostic {
    NotAllPathsReturnValue(NotAllPathsReturnValue),
    Memory(rayc_memory::Diagnostic),
    Borrow(rayc_borrowck::diagnostic::Diagnostic),
}

impl Report for Diagnostic {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        match self {
            Self::NotAllPathsReturnValue(diagnostic) => diagnostic.report(engine).await,
            Self::Memory(diagnostic) => diagnostic.report(engine).await,
            Self::Borrow(diagnostic) => diagnostic.report(engine).await,
        }
    }
}
