#![allow(missing_docs)]

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use clap::Parser;
use insta::assert_snapshot;
use rayc_target::Arguments;

// Each fixture is a project directory with a `ray.toml` manifest. The project
// is checked from its `src` directory when it has one, so that the manifest is
// found in a parent directory.
#[test_generator::test_resources("compiler/e2e/test/project/**/ray.toml")]
fn main(resource: &str) {
    stacker::maybe_grow(3 * 1024 * 1024, 8 * 1024 * 1024, || {
        // diagnostics display paths relative to the current directory, which
        // is canonicalized like the project paths the compiler reports
        let workspace = dunce::canonicalize(env!("RAYC_CARGO_WORKSPACE_DIR")).unwrap();
        std::env::set_current_dir(&workspace).unwrap();

        let project_directory = Path::new(resource).parent().unwrap().to_path_buf();

        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_stack_size(if cfg!(debug_assertions) {
                8 * 1024 * 1024
            } else {
                2 * 1024 * 1024
            })
            .build()
            .expect("failed to create Tokio runtime")
            .block_on(test(&workspace, &project_directory));
    });
}

async fn test(workspace: &Path, project_directory: &Path) {
    let source_directory = project_directory.join("src");
    let start = if source_directory.is_dir() { source_directory } else { project_directory.into() };

    // keep the incremental compilation data out of the fixture
    let incremental_directory = tempfile::tempdir().unwrap();
    let arguments = check_arguments(&start, incremental_directory.path());

    let mut err_writer = Vec::new();
    let mut out_writer = Vec::new();
    let _ = rayc_driver::run(arguments, &mut err_writer, &mut out_writer).await;

    let stderr = String::from_utf8(err_writer).unwrap();
    let stdout = String::from_utf8(out_writer).unwrap();
    let rendered = format!("stderr:\n{stderr}\n\nstdout:\n{stdout}");

    let mut settings = insta::Settings::clone_current();

    // Manifest errors name absolute paths; make them independent of the
    // checkout location.
    settings.add_filter(&escape_regex(&workspace.to_string_lossy()), "[WORKSPACE]");
    // Convert Windows paths to Unix paths.
    settings.add_filter(r"\\\\?([\w\d.])", "/$1");
    // Convert CRLF to LF.
    settings.add_filter(r"\r\n", "\n");

    settings.set_snapshot_path(workspace.join(project_directory));
    settings.set_prepend_module_to_snapshot(false);
    settings.remove_snapshot_suffix();
    let _guard = settings.bind_to_scope();

    assert_snapshot!("snapshot", rendered);
}

fn check_arguments(start: &Path, incremental_directory: &Path) -> Arguments {
    let mut arguments = Arguments::parse_from([
        OsString::from("rayc"),
        OsString::from("check"),
        start.as_os_str().to_owned(),
        OsString::from("--no-fancy"),
        OsString::from("--target-seed"),
        OsString::from("0"),
        OsString::from("--inc"),
        PathBuf::from(incremental_directory).into_os_string(),
    ]);
    arguments.enable_ir_verification();
    arguments
}

fn escape_regex(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        if r"\.+*?()|[]{}^$#&-~".contains(character) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}
