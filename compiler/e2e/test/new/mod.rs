#![allow(missing_docs)]

use std::{ffi::OsString, future::Future, path::Path, process::Command};

use clap::Parser;
use rayc_target::{Arguments, Cli};

// A new binary project builds without changes into a program that greets.
#[test]
fn binary_project_runs() {
    block_on(async {
        let temporary_directory = tempfile::tempdir().unwrap();
        let project_directory = temporary_directory.path().join("hello");

        let status = create_project(&project_directory, &[]);
        assert_eq!(
            status,
            format!(
                "[info]: created binary project `hello` in '{}'\n\n",
                project_directory.display()
            )
        );

        // the executable is written to the output directory of the project
        let stderr = compile("build", &project_directory).await;
        assert_eq!(stderr, "");

        let executable_path =
            project_directory.join("target").join(format!("hello{}", std::env::consts::EXE_SUFFIX));
        let output = Command::new(&executable_path).output().unwrap();

        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap(), "Hello, world!\n");
    });
}

// A new library project checks without diagnostics.
#[test]
fn library_project_checks() {
    block_on(async {
        let temporary_directory = tempfile::tempdir().unwrap();
        let project_directory = temporary_directory.path().join("shapes");

        let status = create_project(&project_directory, &["--lib"]);
        assert_eq!(
            status,
            format!(
                "[info]: created library project `shapes` in '{}'\n\n",
                project_directory.display()
            )
        );

        let stderr = compile("check", &project_directory).await;
        assert_eq!(stderr, "");
    });
}

// An existing directory is never overwritten.
#[test]
fn existing_directory_is_refused() {
    let temporary_directory = tempfile::tempdir().unwrap();

    let status = create_project(temporary_directory.path(), &["--name", "app"]);
    assert_eq!(
        status,
        format!(
            "[error]: destination '{}' already exists\n\n",
            temporary_directory.path().display()
        )
    );
    assert_eq!(std::fs::read_dir(temporary_directory.path()).unwrap().count(), 0);
}

fn create_project(directory: &Path, flags: &[&str]) -> String {
    let arguments = ["rayc", "new", "--no-fancy"]
        .into_iter()
        .map(OsString::from)
        .chain(std::iter::once(directory.as_os_str().to_owned()))
        .chain(flags.iter().map(OsString::from));
    let Cli::New(arguments) = Cli::parse_from(arguments) else {
        panic!("`rayc new` parses as the `new` command");
    };

    let mut err_writer = Vec::new();
    let _ = rayc_driver::create_project(&arguments, &mut err_writer);

    String::from_utf8(err_writer).unwrap()
}

async fn compile(command: &str, project_directory: &Path) -> String {
    let mut arguments = Arguments::parse_from([
        OsString::from("rayc"),
        OsString::from(command),
        project_directory.as_os_str().to_owned(),
        OsString::from("--no-fancy"),
        OsString::from("--target-seed"),
        OsString::from("0"),
    ]);
    arguments.enable_ir_verification();

    let mut err_writer = Vec::new();
    let mut out_writer = Vec::new();
    let _ = rayc_driver::run(arguments, &mut err_writer, &mut out_writer).await;

    String::from_utf8(err_writer).unwrap()
}

fn block_on(future: impl Future<Output = ()>) {
    stacker::maybe_grow(3 * 1024 * 1024, 8 * 1024 * 1024, || {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_stack_size(if cfg!(debug_assertions) {
                8 * 1024 * 1024
            } else {
                2 * 1024 * 1024
            })
            .build()
            .expect("failed to create Tokio runtime")
            .block_on(future);
    });
}
