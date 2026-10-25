#![allow(missing_docs)]

use std::path::{Path, PathBuf};

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

/// A rendered diagnostic, anchored to the fixture line its primary highlight
/// starts on, or `None` when it points at another file or nowhere.
struct Diagnostic {
    line: Option<usize>,
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
            let line = diagnostic.primary_highlight().and_then(|highlight| {
                let span = highlight.span();
                let source_file = source_map.0.get(&span.source_id)?;

                (dunce::canonicalize(source_file.path()).ok()? == fixture_path)
                    .then(|| source_file.byte_to_line(span.start) + 1)
            });

            Diagnostic { line, text: render_plain(diagnostic, &source_map).trim_end().to_owned() }
        })
        .collect()
}

/// Renders the fixture source with every diagnostic placed right below the
/// line it points at, so the snapshot reads without the `main.ray` beside it.
/// Diagnostics pointing at other files or nowhere follow the source.
fn render(source: &str, diagnostics: &[Diagnostic]) -> String {
    use std::fmt::Write;

    let source_lines = source.lines().collect::<Vec<_>>();
    let width = source_lines.len().to_string().len();

    // diagnostics past the end of the fixture, such as at its end of file,
    // are anchored to its last line
    let anchor = |diagnostic: &Diagnostic| {
        diagnostic.line.map(|line| line.min(source_lines.len())).filter(|line| *line > 0)
    };

    let mut rendered = String::new();
    for (index, source_line) in source_lines.iter().enumerate() {
        let number = index + 1;
        writeln!(rendered, "{}", format!("{number:>width$} │ {source_line}").trim_end()).unwrap();

        // pad the diagnostics with an empty gutter line above, between, and
        // below them
        let mut anchored =
            diagnostics.iter().filter(|diagnostic| anchor(diagnostic) == Some(number)).peekable();
        if anchored.peek().is_some() {
            writeln!(rendered, "{:width$} ┆", "").unwrap();
        }
        for diagnostic in anchored {
            for line in diagnostic.text.lines() {
                writeln!(rendered, "{}", format!("{:width$} ┆ {line}", "").trim_end()).unwrap();
            }
            writeln!(rendered, "{:width$} ┆", "").unwrap();
        }
    }

    let others = diagnostics.iter().filter(|diagnostic| anchor(diagnostic).is_none());
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
