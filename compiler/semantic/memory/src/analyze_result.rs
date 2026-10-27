use rayc_handler::Handler2;
use rayc_semantic_element::deferred_marker::DeferredMarker;

use crate::Diagnostic;

/// Result of analyzing a function for memory safety, including the diagnostics
/// found and deferred marker predicates.
#[derive(Debug, Default)]
pub struct AnalyzeResult {
    diagnostics: Vec<Diagnostic>,
    deferred_markers: Vec<DeferredMarker>,
}

impl AnalyzeResult {
    #[must_use]
    pub fn new() -> Self { Self::default() }

    pub fn extend_deferred_markers(&mut self, markers: impl IntoIterator<Item = DeferredMarker>) {
        self.deferred_markers.extend(markers);
    }

    pub fn extend_diagnostics(&mut self, diagnostics: impl IntoIterator<Item = Diagnostic>) {
        self.diagnostics.extend(diagnostics);
    }

    #[must_use]
    pub fn into_parts(self) -> (Vec<Diagnostic>, Vec<DeferredMarker>) {
        (self.diagnostics, self.deferred_markers)
    }
}

impl Handler2<Diagnostic> for AnalyzeResult {
    fn receive(&mut self, error: Diagnostic) { self.diagnostics.push(error); }
}
