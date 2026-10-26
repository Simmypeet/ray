#![allow(missing_docs)]

use std::{
    ops::Range,
    path::{Path, PathBuf},
};

use insta::assert_snapshot;
use rayc_driver::{
    Session,
    term::{ReportTerm, render_plain},
};
use rayc_target::{Arguments, Input};
use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt, util::SubscriberInitExt};

#[test_generator::test_resources("compiler/e2e/test/check/**/main.ray")]
fn main(resource: &str) {
    stacker::maybe_grow(3 * 1024 * 1024, 8 * 1024 * 1024, || {
        let _ = tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_thread_ids(true)
                    .with_thread_names(true)
                    .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE)
                    .with_filter(
                        EnvFilter::try_from_env("RAYC_LOG").unwrap_or_else(|_| "ERROR".into()),
                    ),
            )
            .try_init();

        let file_path = PathBuf::from(resource);
        std::env::set_current_dir(env!("RAYC_CARGO_WORKSPACE_DIR")).unwrap();

        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_stack_size(
                // in the debug build, the stack size of the future is not
                // optimized well, so we need a larger stack size
                {
                    #[cfg(debug_assertions)]
                    {
                        8 * 1024 * 1024 // 8MB stack for debug build
                    }
                    #[cfg(not(debug_assertions))]
                    {
                        2 * 1024 * 1024 // 2MB stack default
                    }
                },
            )
            .build()
            .expect("Failed to create Tokio runtime")
            .block_on(test(&file_path));
    });
}

/// A rendered diagnostic with the byte range of its primary highlight in the
/// fixture, or `None` when it points at another file or nowhere.
struct Diagnostic {
    span: Option<Range<usize>>,
    text: String,
}

/// Checks the fixture and renders every diagnostic of the `check` query, its
/// notes included.
async fn check(file_path: &Path) -> Vec<Diagnostic> {
    let input =
        Input::builder().path(file_path.to_path_buf()).target_seed(0).verify_ir(true).build();
    let arguments = Arguments::new_check(input);

    let mut err_writer = Vec::new();
    let session = Session::new(&arguments, &mut ReportTerm::new(&mut err_writer, false))
        .await
        .unwrap_or_else(|| panic!("{}", String::from_utf8_lossy(&err_writer)));

    let source_map = session.source_map().await;
    let fixture_path = dunce::canonicalize(file_path).unwrap();

    session
        .check()
        .await
        .iter()
        .map(|diagnostic| {
            let span = diagnostic.primary_highlight().and_then(|highlight| {
                let span = highlight.span();
                let source_file = source_map.0.get(&span.source_id)?;

                (dunce::canonicalize(source_file.path()).ok()? == fixture_path)
                    .then_some(span.start..span.end)
            });

            Diagnostic { span, text: render_plain(diagnostic, &source_map).trim_end().to_owned() }
        })
        .collect()
}

/// Renders the fixture source with every diagnostic placed right below the
/// line it points at, so the snapshot reads without the `main.ray` beside it.
/// The line is underlined with `^` where the primary highlight points at, up to
/// the end of the line when the highlight spans further. Diagnostics pointing
/// at other files or nowhere follow the source.
fn render(source: &str, diagnostics: &[Diagnostic]) -> String {
    use std::fmt::Write;

    // the byte offset each line starts at, with the line without its break
    let source_lines = source
        .split_inclusive('\n')
        .scan(0, |start, line| {
            let line_start = *start;
            *start += line.len();
            Some((line_start, line.trim_end_matches(['\n', '\r'])))
        })
        .collect::<Vec<_>>();
    let width = source_lines.len().to_string().len();

    // the 0-indexed line a diagnostic is anchored to and the character columns
    // it underlines there; diagnostics past the end of the fixture, such as at
    // its end of file, are anchored past the end of its last line
    let anchor = |diagnostic: &Diagnostic| {
        let span = diagnostic.span.as_ref()?;
        let index =
            source_lines.partition_point(|(start, _)| *start <= span.start).checked_sub(1)?;
        let (line_start, line) = source_lines[index];

        let column = |byte: usize| {
            line.get(..byte - line_start)
                .map_or(line.chars().count(), |prefix| prefix.chars().count())
        };
        let start = column(span.start);
        let end = column(span.end.max(span.start)).max(start + 1);

        Some((index, start..end))
    };
    let anchors = diagnostics.iter().map(anchor).collect::<Vec<_>>();

    let mut rendered = String::new();
    for (index, (_, source_line)) in source_lines.iter().enumerate() {
        let number = index + 1;
        writeln!(rendered, "{}", format!("{number:>width$} │ {source_line}").trim_end()).unwrap();

        let anchored = diagnostics
            .iter()
            .zip(&anchors)
            .filter_map(|(diagnostic, anchor)| {
                anchor
                    .clone()
                    .filter(|(line, _)| *line == index)
                    .map(|(_, columns)| (diagnostic, columns))
            })
            .collect::<Vec<_>>();

        // underline each distinct highlight once, in the order of the
        // diagnostics
        let mut underlines = Vec::<Range<usize>>::new();
        for (_, columns) in &anchored {
            if !underlines.contains(columns) {
                underlines.push(columns.clone());
            }
        }
        for columns in underlines {
            writeln!(
                rendered,
                "{:width$} │ {}{}",
                "",
                " ".repeat(columns.start),
                "^".repeat(columns.len())
            )
            .unwrap();
        }

        // the diagnostics follow the underlines directly, padded with an
        // empty gutter line between and below them
        for (diagnostic, _) in anchored {
            for line in diagnostic.text.lines() {
                writeln!(rendered, "{}", format!("{:width$} ┆ {line}", "").trim_end()).unwrap();
            }
            writeln!(rendered, "{:width$} ┆", "").unwrap();
        }
    }

    let others = diagnostics
        .iter()
        .zip(&anchors)
        .filter(|(_, anchor)| anchor.is_none())
        .map(|(diagnostic, _)| diagnostic);
    for (index, diagnostic) in others.enumerate() {
        rendered.push_str(if index == 0 { "\nother diagnostics:\n" } else { "\n" });
        writeln!(rendered, "{}", diagnostic.text).unwrap();
    }

    rendered
}

async fn test(file_path: &Path) {
    let source = std::fs::read_to_string(file_path).unwrap();
    let snapshot = render(&source, &check(file_path).await);

    let mut settings = insta::Settings::clone_current();

    // Convert windows paths to Unix Paths.
    settings.add_filter(r"\\\\?([\w\d.])", "/$1");
    // Convert crlf to lf.
    settings.add_filter(r"\r\n", "\n");
    // Replace the IO error message with a generic one.
    settings.add_filter(r": [^(\r\n:]*\(os error (\d+)\)", ": general IO error (os error $1)");

    let full_path = std::fs::canonicalize(file_path).unwrap();
    settings.set_snapshot_path(full_path.parent().unwrap());
    settings.set_prepend_module_to_snapshot(false);
    settings.remove_snapshot_suffix();
    let _guard = settings.bind_to_scope();

    assert_snapshot!("snapshot", snapshot);
}
