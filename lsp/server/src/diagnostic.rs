//! Converts compiler byte offsets and diagnostic groups to LSP diagnostics.

use rayc_diagnostic::{Group, Highlight, Rendered, Severity};
use tower_lsp::lsp_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, Location, Position, Range, Url,
};

fn position(text: &str, offset: usize) -> Position {
    // LSP defaults to UTF-16 code units; compiler spans use UTF-8 bytes.
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let prefix = &text[..offset];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count();
    let column = prefix.rsplit('\n').next().unwrap_or("").encode_utf16().count();
    Position::new(
        u32::try_from(line).unwrap_or(u32::MAX),
        u32::try_from(column).unwrap_or(u32::MAX),
    )
}

fn range(highlight: &Highlight<usize>, text: &str) -> Range {
    Range::new(position(text, highlight.span().start), position(text, highlight.span().end))
}

fn location(
    highlight: &Highlight<usize>,
    uri: &Url,
    text: &str,
    sources: &rayc_symbol_impl::source_map::SourceMap,
) -> Option<Location> {
    let id = highlight.span().source_id;
    let source = sources.0.get(&id)?;
    if id.target_id == rayc_target::TargetID::CORE {
        Some(Location::new(
            Url::parse(rayc_corelib::SOURCE_URI).expect("valid core URI"),
            range(highlight, &source.content()),
        ))
    } else {
        Some(Location::new(uri.clone(), range(highlight, text)))
    }
}

fn append_group(
    group: &Group<usize>,
    uri: &Url,
    text: &str,
    sources: &rayc_symbol_impl::source_map::SourceMap,
    message: &mut String,
    related: &mut Vec<DiagnosticRelatedInformation>,
) {
    if let Some(help) = group.help_message() {
        message.push_str("\nhelp: ");
        message.push_str(help);
    }
    for highlight in group.primary_highlight().into_iter().chain(group.related()) {
        if let Some(label) = highlight.message()
            && let Some(location) = location(highlight, uri, text, sources)
        {
            related.push(DiagnosticRelatedInformation { location, message: label.to_owned() });
        }
    }
}

pub(crate) fn convert(
    diagnostic: &Rendered<usize>,
    uri: &Url,
    text: &str,
    sources: &rayc_symbol_impl::source_map::SourceMap,
) -> Diagnostic {
    let mut message = diagnostic.message().to_owned();
    let mut related = Vec::new();
    append_group(diagnostic.group(), uri, text, sources, &mut message, &mut related);
    for note in diagnostic.notes() {
        message.push_str("\nnote: ");
        message.push_str(note.message());
        if let Some(highlight) = note.primary_highlight()
            && let Some(location) = location(highlight, uri, text, sources)
        {
            related.push(DiagnosticRelatedInformation {
                location,
                message: note.message().to_owned(),
            });
        }
        append_group(note.group(), uri, text, sources, &mut message, &mut related);
    }

    if let Some(highlight) = diagnostic.primary_highlight()
        && highlight.span().source_id.target_id == rayc_target::TargetID::CORE
    {
        message.insert_str(0, "bundled core library: ");
        if let Some(location) = location(highlight, uri, text, sources) {
            related.push(DiagnosticRelatedInformation {
                location,
                message: diagnostic.message().to_owned(),
            });
        }
    }
    Diagnostic {
        range: diagnostic
            .primary_highlight()
            .and_then(|highlight| location(highlight, uri, text, sources))
            .filter(|location| &location.uri == uri)
            .map_or_else(Range::default, |location| location.range),
        severity: Some(match diagnostic.severity() {
            Severity::Info => DiagnosticSeverity::INFORMATION,
            Severity::Warning => DiagnosticSeverity::WARNING,
            Severity::Error => DiagnosticSeverity::ERROR,
        }),
        source: Some("ray".to_owned()),
        message,
        related_information: (!related.is_empty()).then_some(related),
        ..Diagnostic::default()
    }
}
