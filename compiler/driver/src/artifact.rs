//! Compiler artifact generation and ownership.

use std::{
    fmt,
    io::{self, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

use rayc_c2::CTranslationUnitOptions;
use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;
use rayc_target::{TargetID, TargetKind};
use tempfile::{Builder, NamedTempFile, TempPath};

use crate::{
    invocation::Artifact,
    native::{NativeError, NativeOptions, compile_c, launch_executable, link_executable},
};

/// A temporary compiler-owned artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TemporaryArtifact {
    CSource,
    Object,
}

impl TemporaryArtifact {
    const fn description(self) -> &'static str {
        match self {
            Self::CSource => "C source",
            Self::Object => "object",
        }
    }

    const fn suffix(self) -> &'static str {
        match self {
            Self::CSource => ".c",
            Self::Object => ".o",
        }
    }
}

/// An error encountered while producing or launching an artifact.
#[derive(Debug)]
pub(super) enum ArtifactError {
    UnsupportedArtifact { kind: TargetKind },
    MissingOutputPath { kind: TargetKind },
    MissingEntryPoint,
    CreateOutputDirectory { path: PathBuf, source: io::Error },
    CreateOutputStaging { path: PathBuf, source: io::Error },
    CreateTemporaryArtifact { artifact: TemporaryArtifact, output_path: PathBuf, source: io::Error },
    GenerateC { output_path: PathBuf, source: io::Error },
    FlushC { output_path: PathBuf, source: io::Error },
    PersistOutput { path: PathBuf, source: io::Error },
    Native(NativeError),
}

impl fmt::Display for ArtifactError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedArtifact { kind } => write!(
                formatter,
                "artifact kind `{}` is not supported by the current C backend",
                artifact_name(*kind)
            ),
            Self::MissingOutputPath { kind } => write!(
                formatter,
                "could not resolve an output path for artifact kind `{}`",
                artifact_name(*kind)
            ),
            Self::MissingEntryPoint => {
                formatter.write_str("executable artifact is missing its validated entry point")
            }
            Self::CreateOutputDirectory { path, source } => write!(
                formatter,
                "failed to create the output directory '{}': {source}",
                path.display()
            ),
            Self::CreateOutputStaging { path, source } => write!(
                formatter,
                "failed to create a temporary output beside '{}': {source}",
                path.display()
            ),
            Self::CreateTemporaryArtifact { artifact, output_path, source } => write!(
                formatter,
                "failed to create temporary {} input for '{}': {source}",
                artifact.description(),
                output_path.display()
            ),
            Self::GenerateC { output_path, source } => write!(
                formatter,
                "failed to generate C input for '{}': {source}",
                output_path.display()
            ),
            Self::FlushC { output_path, source } => write!(
                formatter,
                "failed to flush C input for '{}': {source}",
                output_path.display()
            ),
            Self::PersistOutput { path, source } => {
                write!(formatter, "failed to persist output '{}': {source}", path.display())
            }
            Self::Native(error) => error.fmt(formatter),
        }
    }
}

impl From<NativeError> for ArtifactError {
    fn from(error: NativeError) -> Self { Self::Native(error) }
}

/// Produces the requested artifact and optionally launches it.
pub(super) async fn execute(
    engine: &TrackedEngine,
    target_id: TargetID,
    artifact: &Artifact,
    entry_point: Option<GlobalSymbolID>,
) -> Result<ExitCode, ArtifactError> {
    let kind = artifact.kind();

    match kind {
        TargetKind::Library | TargetKind::LLvmIR => {
            return Err(ArtifactError::UnsupportedArtifact { kind });
        }
        TargetKind::Executable | TargetKind::Object | TargetKind::C => {}
    }

    let output_path = artifact.output_path().ok_or(ArtifactError::MissingOutputPath { kind })?;

    match kind {
        TargetKind::Executable => {
            let entry_point = entry_point.ok_or(ArtifactError::MissingEntryPoint)?;
            build_executable(
                engine,
                target_id,
                entry_point,
                artifact.native_options(),
                output_path,
            )
            .await?;

            if artifact.should_run() {
                launch_executable(output_path).map_err(ArtifactError::from)
            } else {
                Ok(ExitCode::SUCCESS)
            }
        }
        TargetKind::Object => {
            build_object(engine, target_id, artifact.native_options(), output_path).await?;
            Ok(ExitCode::SUCCESS)
        }
        TargetKind::C => {
            emit_final_c(engine, target_id, output_path).await?;
            Ok(ExitCode::SUCCESS)
        }
        TargetKind::Library | TargetKind::LLvmIR => {
            Err(ArtifactError::UnsupportedArtifact { kind })
        }
    }
}

/// Writes the generated C to `output_path`, and the runtime it must be linked
/// with beside it, at [`runtime_sibling_path`].
///
/// The two stay apart: the headers the runtime includes could conflict with
/// the `extern def`s of the generated unit.
async fn emit_final_c(
    engine: &TrackedEngine,
    target_id: TargetID,
    output_path: &Path,
) -> Result<(), ArtifactError> {
    // both are staged first, so that a failure leaves neither half written
    let mut staging = temporary_output_file(output_path)?;
    write_c(
        engine,
        target_id,
        CTranslationUnitOptions::ordinary(),
        staging.as_file_mut(),
        output_path,
    )
    .await?;

    let runtime_path = runtime_sibling_path(output_path, "c");
    let mut runtime_staging = temporary_output_file(&runtime_path)?;
    write_runtime_c(runtime_staging.as_file_mut(), &runtime_path)?;

    persist_output(staging.into_temp_path(), output_path)?;
    persist_output(runtime_staging.into_temp_path(), &runtime_path)
}

/// Compiles the generated code to `output_path`, and the runtime it must be
/// linked with beside it, at [`runtime_sibling_path`].
async fn build_object(
    engine: &TrackedEngine,
    target_id: TargetID,
    native_options: NativeOptions,
    output_path: &Path,
) -> Result<(), ArtifactError> {
    // both are staged first, so that a failure leaves neither half written
    let staging = temporary_output_path(output_path)?;
    compile_generated_c(
        engine,
        target_id,
        CTranslationUnitOptions::ordinary(),
        native_options,
        &staging,
        output_path,
    )
    .await?;

    let runtime_path = runtime_sibling_path(output_path, "o");
    let runtime_staging = temporary_output_path(&runtime_path)?;
    compile_runtime_c(native_options, &runtime_staging, &runtime_path)?;

    persist_output(staging, output_path)?;
    persist_output(runtime_staging, &runtime_path)
}

/// Compiles the generated code and the runtime, each as a translation unit of
/// its own, and links them into an executable.
async fn build_executable(
    engine: &TrackedEngine,
    target_id: TargetID,
    entry_point: GlobalSymbolID,
    native_options: NativeOptions,
    output_path: &Path,
) -> Result<(), ArtifactError> {
    let executable_staging = temporary_output_path(output_path)?;

    let generated = temporary_artifact(TemporaryArtifact::Object, output_path)?;
    compile_generated_c(
        engine,
        target_id,
        CTranslationUnitOptions::executable(entry_point),
        native_options,
        &generated,
        output_path,
    )
    .await?;

    let runtime = temporary_artifact(TemporaryArtifact::Object, output_path)?;
    compile_runtime_c(native_options, &runtime, output_path)?;

    link_executable(&[&generated, &runtime], &executable_staging, native_options)?;
    persist_output(executable_staging, output_path)
}

/// Compiles the generated translation unit into `object_path`.
async fn compile_generated_c(
    engine: &TrackedEngine,
    target_id: TargetID,
    options: CTranslationUnitOptions,
    native_options: NativeOptions,
    object_path: &Path,
    output_path: &Path,
) -> Result<(), ArtifactError> {
    let c_path = temporary_c_input(engine, target_id, options, output_path).await?;
    compile_c(&c_path, object_path, native_options)?;
    Ok(())
}

/// Compiles the runtime translation unit into `object_path`.
fn compile_runtime_c(
    native_options: NativeOptions,
    object_path: &Path,
    output_path: &Path,
) -> Result<(), ArtifactError> {
    let mut c_file = temporary_artifact_file(TemporaryArtifact::CSource, output_path)?;
    write_runtime_c(c_file.as_file_mut(), output_path)?;
    let c_path = c_file.into_temp_path();
    compile_c(&c_path, object_path, native_options)?;
    Ok(())
}

async fn temporary_c_input(
    engine: &TrackedEngine,
    target_id: TargetID,
    options: CTranslationUnitOptions,
    output_path: &Path,
) -> Result<TempPath, ArtifactError> {
    let mut temporary = temporary_artifact_file(TemporaryArtifact::CSource, output_path)?;
    write_c(engine, target_id, options, temporary.as_file_mut(), output_path).await?;
    Ok(temporary.into_temp_path())
}

async fn write_c(
    engine: &TrackedEngine,
    target_id: TargetID,
    options: CTranslationUnitOptions,
    writer: &mut impl Write,
    output_path: &Path,
) -> Result<(), ArtifactError> {
    rayc_c2::write_c_translation_unit(engine, target_id, options, writer).await.map_err(
        |source| ArtifactError::GenerateC { output_path: output_path.to_owned(), source },
    )?;
    writer
        .flush()
        .map_err(|source| ArtifactError::FlushC { output_path: output_path.to_owned(), source })
}

fn write_runtime_c(writer: &mut impl Write, output_path: &Path) -> Result<(), ArtifactError> {
    rayc_c2::write_c_runtime(writer).map_err(|source| ArtifactError::GenerateC {
        output_path: output_path.to_owned(),
        source,
    })?;
    writer
        .flush()
        .map_err(|source| ArtifactError::FlushC { output_path: output_path.to_owned(), source })
}

/// Where `--emit c` and `--emit obj` write the runtime: beside `output_path`,
/// with its extension replaced by `runtime.<extension>`, so `out.c` gets
/// `out.runtime.c` and `out.o` gets `out.runtime.o`.
fn runtime_sibling_path(output_path: &Path, extension: &str) -> PathBuf {
    output_path.with_extension(format!("runtime.{extension}"))
}

fn temporary_output_file(output_path: &Path) -> Result<NamedTempFile, ArtifactError> {
    // the output is staged beside its final path, so its directory must exist
    let parent = output_parent(output_path);
    std::fs::create_dir_all(parent).map_err(|source| ArtifactError::CreateOutputDirectory {
        path: parent.to_owned(),
        source,
    })?;

    Builder::new().prefix(".rayc-").tempfile_in(output_parent(output_path)).map_err(|source| {
        ArtifactError::CreateOutputStaging { path: output_path.to_owned(), source }
    })
}

fn temporary_output_path(output_path: &Path) -> Result<TempPath, ArtifactError> {
    temporary_output_file(output_path).map(NamedTempFile::into_temp_path)
}

fn temporary_artifact(
    artifact: TemporaryArtifact,
    output_path: &Path,
) -> Result<TempPath, ArtifactError> {
    temporary_artifact_file(artifact, output_path).map(NamedTempFile::into_temp_path)
}

fn temporary_artifact_file(
    artifact: TemporaryArtifact,
    output_path: &Path,
) -> Result<NamedTempFile, ArtifactError> {
    Builder::new().prefix("rayc-").suffix(artifact.suffix()).tempfile().map_err(|source| {
        ArtifactError::CreateTemporaryArtifact {
            artifact,
            output_path: output_path.to_owned(),
            source,
        }
    })
}

fn persist_output(staging: TempPath, output_path: &Path) -> Result<(), ArtifactError> {
    staging.persist(output_path).map_err(|error| ArtifactError::PersistOutput {
        path: output_path.to_owned(),
        source: error.error,
    })
}

fn output_parent(output_path: &Path) -> &Path {
    output_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

const fn artifact_name(kind: TargetKind) -> &'static str {
    match kind {
        TargetKind::Executable => "bin",
        TargetKind::Library => "lib",
        TargetKind::LLvmIR => "llvm",
        TargetKind::Object => "obj",
        TargetKind::C => "c",
    }
}
