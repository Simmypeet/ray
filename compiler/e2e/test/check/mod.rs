#![allow(missing_docs)]

use std::path::{Path, PathBuf};

use clap::Parser;
use insta::assert_snapshot;
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

/// Runs the compiler and returns its stderr and stdout.
async fn run(arguments: Arguments) -> (String, String) {
    let mut err_writer = Vec::new();
    let mut out_writer = Vec::new();

    let _ = rayc_driver::run(arguments, &mut err_writer, &mut out_writer).await;

    (String::from_utf8(err_writer).unwrap(), String::from_utf8(out_writer).unwrap())
}

/// A rendered diagnostic, anchored to the fixture line its primary span
/// starts on, or `None` when it points at another file or nowhere.
struct Diagnostic<'a> {
    line: Option<usize>,
    text: Vec<&'a str>,
}

/// Splits the rendered stderr into diagnostics, each starting at a
/// `[severity]: message` header.
fn parse_diagnostics<'a>(
    stderr: &'a str,
    file_path: &Path,
    line_count: usize,
) -> Vec<Diagnostic<'a>> {
    let is_header = |line: &str| {
        line.strip_prefix('[')
            .and_then(|rest| rest.split_once("]: "))
            .is_some_and(|(severity, _)| severity.chars().all(char::is_alphabetic))
    };
    let fixture_path = file_path.to_string_lossy().replace('\\', "/");

    let mut diagnostics: Vec<Diagnostic<'a>> = Vec::new();
    for line in stderr.lines() {
        match diagnostics.last_mut() {
            Some(diagnostic) if !is_header(line) => diagnostic.text.push(line),
            _ => diagnostics.push(Diagnostic { line: None, text: vec![line] }),
        }
    }

    for diagnostic in &mut diagnostics {
        while diagnostic.text.last().is_some_and(|line| line.trim().is_empty()) {
            diagnostic.text.pop();
        }

        // the primary location is the first `╭▸ path:line:column`
        let location =
            diagnostic.text.iter().find_map(|line| line.trim_start().strip_prefix("╭▸ "));
        diagnostic.line = location.and_then(|location| {
            let mut parts = location.rsplitn(3, ':');
            let (_, line, path) = (parts.next()?, parts.next()?, parts.next()?);
            (path.replace('\\', "/") == fixture_path)
                .then(|| line.parse::<usize>().ok())
                .flatten()
                .filter(|line| *line > 0)
                .map(|line| line.min(line_count))
        });
    }

    diagnostics
}

/// Renders the fixture source with every diagnostic placed right below the
/// line it points at, so the snapshot reads without the `main.ray` beside it.
/// Diagnostics pointing at other files or nowhere follow the source.
fn render(source: &str, stderr: &str, stdout: &str, file_path: &Path) -> String {
    use std::fmt::Write;

    let source_lines = source.lines().collect::<Vec<_>>();
    let diagnostics = parse_diagnostics(stderr, file_path, source_lines.len());
    let width = source_lines.len().to_string().len();

    let mut rendered = String::new();
    for (index, source_line) in source_lines.iter().enumerate() {
        let number = index + 1;
        writeln!(rendered, "{}", format!("{number:>width$} │ {source_line}").trim_end()).unwrap();

        // pad the diagnostics with an empty gutter line above, between, and
        // below them
        let mut anchored =
            diagnostics.iter().filter(|diagnostic| diagnostic.line == Some(number)).peekable();
        if anchored.peek().is_some() {
            writeln!(rendered, "{:width$} ┆", "").unwrap();
        }
        for diagnostic in anchored {
            for line in &diagnostic.text {
                writeln!(rendered, "{}", format!("{:width$} ┆ {line}", "").trim_end()).unwrap();
            }
            writeln!(rendered, "{:width$} ┆", "").unwrap();
        }
    }

    let others = diagnostics.iter().filter(|diagnostic| diagnostic.line.is_none());
    for (index, diagnostic) in others.enumerate() {
        rendered.push_str(if index == 0 { "\nother diagnostics:\n" } else { "\n" });
        for line in &diagnostic.text {
            writeln!(rendered, "{line}").unwrap();
        }
    }

    if !stdout.is_empty() {
        write!(rendered, "\nstdout:\n{stdout}").unwrap();
    }

    rendered
}

async fn test(file_path: &Path) {
    let source = std::fs::read_to_string(file_path).unwrap();

    let input = || {
        Input::builder()
            .file(file_path.to_path_buf())
            .target_seed(0)
            .verify_ir(true)
            .fancy(false)
            .build()
    };

    // read for the custom cli interface starting with `##` comment
    let mut arguments = source.lines().next().map(str::to_owned).map_or_else(
        || Arguments::new_check(input()),
        |mut first_line| {
            if first_line.starts_with("##") {
                // replace the `{}` with the file path (if any) first_line =
                first_line = first_line.replace("$FILE_PATH", file_path.to_string_lossy().as_ref());

                first_line.strip_prefix("##").unwrap();

                Arguments::parse_from(first_line.split_whitespace())
            } else {
                Arguments::new_check(input())
            }
        },
    );
    arguments.enable_ir_verification();

    let (stderr, stdout) = run(arguments).await;
    let snapshot = render(&source, &stderr, &stdout, file_path);

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
