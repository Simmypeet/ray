//! Converts compiler diagnostics to LSP diagnostics.
//!
//! A compiler diagnostic nests notes, help messages and labelled highlights,
//! while an LSP diagnostic has one range, one message and a flat list of
//! related locations. The primary highlight becomes the range, the texts are
//! joined into the message, and every other labelled highlight becomes a
//! related location. A note highlighted in the project becomes a diagnostic of
//! its own with information severity.

use std::path::Path;

use rayc_diagnostic::{Group, Highlight, Rendered, Severity};
use rayc_symbol_impl::source_map::SourceMap;
use rayc_target::TargetID;
use tower_lsp::lsp_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, Location, Range, Url,
};

use crate::{overlay::Overlay, position::range_of};

/// Places compiler diagnostics in the documents of a project.
pub(crate) struct Locator<'a> {
    sources: &'a SourceMap,
    overlay: &'a Overlay,

    /// The document showing diagnostics that have no location in the project.
    root_file: Url,
}

impl<'a> Locator<'a> {
    pub(crate) fn new(sources: &'a SourceMap, overlay: &'a Overlay, root_file: &Path) -> Self {
        let root_file = overlay.document_uri(root_file).expect("the root file path is absolute");
        Self { sources, overlay, root_file }
    }

    fn is_in_core(highlight: &Highlight<usize>) -> bool {
        highlight.span().source_id.target_id == TargetID::CORE
    }

    /// Returns the location of `highlight`, or `None` if its source file was
    /// not loaded.
    fn location(&self, highlight: &Highlight<usize>) -> Option<Location> {
        let id = highlight.span().source_id;
        let source = self.sources.0.get(&id)?;
        let uri = self.overlay.source_uri(id.target_id, source.path())?;

        Some(Location::new(uri, range_of(source, highlight.span())))
    }

    /// Relates `highlight` to the diagnostic under its label, or under
    /// `default_label` if it has none.
    ///
    /// The label of a highlight that cannot be located is kept in the message
    /// instead.
    fn relate(
        &self,
        highlight: &Highlight<usize>,
        default_label: Option<&str>,
        message: &mut String,
        related: &mut Vec<DiagnosticRelatedInformation>,
    ) {
        let Some(location) = self.location(highlight) else {
            if let Some(label) = highlight.message() {
                message.push('\n');
                message.push_str(label);
            }
            return;
        };

        if let Some(label) = highlight.message().or(default_label) {
            related.push(DiagnosticRelatedInformation { location, message: label.to_owned() });
        }
    }

    /// Appends the help message and the related highlights of `group`, but not
    /// its primary highlight.
    fn append_details(
        &self,
        group: &Group<usize>,
        message: &mut String,
        related: &mut Vec<DiagnosticRelatedInformation>,
    ) {
        if let Some(help) = group.help_message() {
            message.push_str("\nhelp: ");
            message.push_str(help);
        }
        for highlight in group.related() {
            self.relate(highlight, None, message, related);
        }
    }

    /// Returns the first highlight of `diagnostic` other than its primary one
    /// that can be shown in the project.
    fn first_project_location(&self, diagnostic: &Rendered<usize>) -> Option<Location> {
        diagnostic
            .related()
            .iter()
            .chain(
                diagnostic
                    .notes()
                    .iter()
                    .flat_map(|note| note.primary_highlight().into_iter().chain(note.related())),
            )
            .filter(|highlight| !Self::is_in_core(highlight))
            .find_map(|highlight| self.location(highlight))
    }

    /// Converts `diagnostic` into LSP diagnostics, each paired with the
    /// document it is shown in.
    ///
    /// The first diagnostic is the main one, followed by one for each note
    /// highlighted in the project.
    pub(crate) fn convert(&self, diagnostic: &Rendered<usize>) -> Vec<(Url, Diagnostic)> {
        let mut message = diagnostic.message().to_owned();
        let mut related = Vec::new();

        // the primary highlight is the range of the diagnostic, so its label
        // joins the message instead of relating it to itself
        let primary = diagnostic.primary_highlight();
        if let Some(label) = primary.and_then(Highlight::message) {
            message.push('\n');
            message.push_str(label);
        }
        self.append_details(diagnostic.group(), &mut message, &mut related);

        // a note highlighted in the project is shown as its own diagnostic and
        // related under its message; any other note joins the message, and its
        // primary highlight is related under the note's message unless it has
        // its own label
        let mut shown_notes = Vec::new();
        for note in diagnostic.notes() {
            let location = note
                .primary_highlight()
                .filter(|highlight| !Self::is_in_core(highlight))
                .and_then(|highlight| self.location(highlight));
            if let Some(location) = location {
                related.push(DiagnosticRelatedInformation {
                    location: location.clone(),
                    message: note.message().to_owned(),
                });
                shown_notes.push((note, location));
                continue;
            }

            message.push_str("\nnote: ");
            message.push_str(note.message());
            if let Some(highlight) = note.primary_highlight() {
                self.relate(highlight, Some(note.message()), &mut message, &mut related);
            }
            self.append_details(note.group(), &mut message, &mut related);
        }

        // a primary highlight in the bundled core library cannot be shown
        // there, so the diagnostic is shown where the project is involved and
        // links to the core location
        let location = match primary {
            Some(highlight) if Self::is_in_core(highlight) => {
                message.insert_str(0, "bundled core library: ");
                if let Some(location) = self.location(highlight) {
                    related.push(DiagnosticRelatedInformation {
                        location,
                        message: diagnostic.message().to_owned(),
                    });
                }
                self.first_project_location(diagnostic)
            }
            Some(highlight) => self.location(highlight),
            None => self.first_project_location(diagnostic),
        };

        // a note links back to the main diagnostic
        let notes = shown_notes
            .into_iter()
            .map(|(note, note_location)| {
                let mut message = note.message().to_owned();
                let mut related = Vec::new();
                if let Some(label) = note.primary_highlight().and_then(Highlight::message) {
                    message.push('\n');
                    message.push_str(label);
                }
                self.append_details(note.group(), &mut message, &mut related);
                if let Some(location) = location.clone() {
                    related.push(DiagnosticRelatedInformation {
                        location,
                        message: diagnostic.message().to_owned(),
                    });
                }

                (
                    note_location.uri,
                    Self::lsp_diagnostic(
                        note_location.range,
                        DiagnosticSeverity::INFORMATION,
                        message,
                        related,
                    ),
                )
            })
            .collect::<Vec<_>>();

        // without any location in the project, the diagnostic is shown at the
        // start of the root file
        let (uri, range) = location.map_or_else(
            || (self.root_file.clone(), Range::default()),
            |location| (location.uri, location.range),
        );
        let severity = match diagnostic.severity() {
            Severity::Info => DiagnosticSeverity::INFORMATION,
            Severity::Warning => DiagnosticSeverity::WARNING,
            Severity::Error => DiagnosticSeverity::ERROR,
        };

        std::iter::once((uri, Self::lsp_diagnostic(range, severity, message, related)))
            .chain(notes)
            .collect()
    }

    fn lsp_diagnostic(
        range: Range,
        severity: DiagnosticSeverity,
        message: String,
        related: Vec<DiagnosticRelatedInformation>,
    ) -> Diagnostic {
        Diagnostic {
            range,
            severity: Some(severity),
            source: Some("ray".to_owned()),
            message,
            related_information: (!related.is_empty()).then_some(related),
            ..Diagnostic::default()
        }
    }
}
