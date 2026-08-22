#![allow(missing_docs)]

use std::{
    ffi::OsString,
    path::Path,
    process::{Command, Output},
};

use clap::Parser;
use insta::assert_snapshot;
use rayc_target::Arguments;

#[test_generator::test_resources("compiler/e2e/test/run/**/main.ray")]
fn main(resource: &str) {
    stacker::maybe_grow(3 * 1024 * 1024, 8 * 1024 * 1024, || {
        let workspace = Path::new(env!("RAYC_CARGO_WORKSPACE_DIR"));
        let file_path = std::fs::canonicalize(workspace.join(resource)).unwrap();

        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_stack_size(if cfg!(debug_assertions) {
                8 * 1024 * 1024
            } else {
                2 * 1024 * 1024
            })
            .build()
            .expect("failed to create Tokio runtime")
            .block_on(run_fixture(&file_path));
    });
}

async fn run_fixture(file_path: &Path) {
    let temporary_directory = tempfile::tempdir().unwrap();
    let executable_path =
        temporary_directory.path().join(format!("program{}", std::env::consts::EXE_SUFFIX));
    let arguments = build_arguments(file_path, &executable_path);
    let mut compiler_stderr = Vec::new();
    let mut compiler_stdout = Vec::new();

    let _ = rayc_driver::run(arguments, &mut compiler_stderr, &mut compiler_stdout).await;

    let compiler_stderr = String::from_utf8_lossy(&compiler_stderr);
    assert!(
        !compiler_stderr.contains("[error]"),
        "rayc emitted a compiler error for '{}':\n{compiler_stderr}",
        file_path.display()
    );
    assert!(
        executable_path.is_file(),
        "rayc did not produce executable '{}' for '{}'",
        executable_path.display(),
        file_path.display()
    );

    let output = Command::new(&executable_path)
        .output()
        .expect("failed to execute the compiled Ray program");

    let rendered = render_output(&output);

    let mut settings = insta::Settings::clone_current();

    // Convert Windows paths to Unix paths.
    settings.add_filter(r"\\\\?([\w\d.])", "/$1");
    // Convert CRLF to LF.
    settings.add_filter(r"\r\n", "\n");

    settings.set_snapshot_path(file_path.parent().unwrap());
    settings.set_prepend_module_to_snapshot(false);
    settings.remove_snapshot_suffix();
    let _guard = settings.bind_to_scope();

    assert_snapshot!("snapshot", rendered);
}

fn build_arguments(file_path: &Path, executable_path: &Path) -> Arguments {
    Arguments::parse_from([
        OsString::from("rayc"),
        OsString::from("build"),
        file_path.as_os_str().to_owned(),
        OsString::from("--emit"),
        OsString::from("bin"),
        OsString::from("--no-fancy"),
        OsString::from("--target-seed"),
        OsString::from("0"),
        OsString::from("-o"),
        executable_path.as_os_str().to_owned(),
    ])
}

fn render_output(output: &Output) -> String {
    let exit_code =
        output.status.code().map_or_else(|| "<signal>".to_owned(), |code| code.to_string());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    format!("exit_code: {exit_code}\n\nstdout:\n{stdout}\n\nstderr:\n{stderr}")
}
