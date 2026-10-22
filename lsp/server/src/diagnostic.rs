//! Converts compiler diagnostics to LSP diagnostics.
//!
//! A compiler diagnostic nests notes, help messages and labelled highlights,
//! while an LSP diagnostic has one range, one message and a flat list of
//! related locations. The primary highlight becomes the range, the texts are
//! joined into the message, and every other labelled highlight becomes a
//! related location.

use std::path::Path;

use rayc_diagnostic::{Group, Highlight, Rendered, Severity};
use rayc_source_file::SourceFile;
use rayc_symbol_impl::source_map::SourceMap;
use rayc_target::TargetID;
use tower_lsp::lsp_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, Location, Position, Range, Url,
};

use crate::overlay::Overlay;

/// Places compiler diagnostics in the documents of a project.
pub(crate) struct Locator<'a> {
    sources: &'a SourceMap,
    overlay: &'a Overlay,

    /// The document showing diagnostics that have no location in the project.
    root_file: Url,
}

impl<'a> Locator<'a> {
    pub(crate) fn new(sources: &'a SourceMap, overlay: &'a Overlay, root_file: &Path) -> Self {
        let root_file =
            Self::document_uri(overlay, root_file).expect("the root file path is absolute");
        Self { sources, overlay, root_file }
    }

    /// Returns the URI of the document at `path`, preferring the one the
    /// editor opened it with.
    fn document_uri(overlay: &Overlay, path: &Path) -> Option<Url> {
        overlay.document(path).map(|(uri, _)| uri).or_else(|| Url::from_file_path(path).ok())
    }

    fn is_in_core(highlight: &Highlight<usize>) -> bool {
        highlight.span().source_id.target_id == TargetID::CORE
    }

    /// Returns the location of `highlight`, or `None` if its source file was
    /// not loaded.
    fn location(&self, highlight: &Highlight<usize>) -> Option<Location> {
        let id = highlight.span().source_id;
        let source = self.sources.0.get(&id)?;

        // the bundled core library has no file; the editor asks the server
        // for its text by this URI
        let uri = if Self::is_in_core(highlight) {
            Url::parse(rayc_corelib::SOURCE_URI).expect("valid core URI")
        } else {
            Self::document_uri(self.overlay, source.path())?
        };

        let range = Range::new(
            Self::position(source, highlight.span().start),
            Self::position(source, highlight.span().end),
        );

        Some(Location::new(uri, range))
    }

    fn position(source: &SourceFile, byte_index: usize) -> Position {
        let location = source.get_utf16_location_from_byte_index(byte_index);
        Position::new(
            u32::try_from(location.line).unwrap_or(u32::MAX),
            u32::try_from(location.column).unwrap_or(u32::MAX),
        )
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

    /// Converts `diagnostic`, returning the document it is shown in.
    pub(crate) fn convert(&self, diagnostic: &Rendered<usize>) -> (Url, Diagnostic) {
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

        // a note's primary highlight is related under the note's message
        // unless it has its own label
        for note in diagnostic.notes() {
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

        // without any location in the project, the diagnostic is shown at the
        // start of the root file
        let (uri, range) = location.map_or_else(
            || (self.root_file.clone(), Range::default()),
            |location| (location.uri, location.range),
        );

        let diagnostic = Diagnostic {
            range,
            severity: Some(match diagnostic.severity() {
                Severity::Info => DiagnosticSeverity::INFORMATION,
                Severity::Warning => DiagnosticSeverity::WARNING,
                Severity::Error => DiagnosticSeverity::ERROR,
            }),
            source: Some("ray".to_owned()),
            message,
            related_information: (!related.is_empty()).then_some(related),
            ..Diagnostic::default()
        };

        (uri, diagnostic)
    }
}
