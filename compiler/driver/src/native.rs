//! Native compiler and executable process handling.

use std::{
    fmt, io,
    path::{Path, PathBuf},
    process::{Command, ExitCode, ExitStatus},
};

use rayc_target::OptimizationLevel;

/// A native compilation phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NativePhase {
    Compilation,
    Linking,
}

impl NativePhase {
    const fn description(self) -> &'static str {
        match self {
            Self::Compilation => "C compilation",
            Self::Linking => "linking",
        }
    }
}

/// An error from the native toolchain or a launched Ray executable.
#[derive(Debug)]
pub(super) enum NativeError {
    CompilerSpawn {
        phase: NativePhase,
        input_path: PathBuf,
        output_path: PathBuf,
        source: io::Error,
    },
    CompilerFailure {
        phase: NativePhase,
        input_path: PathBuf,
        output_path: PathBuf,
        status: ExitStatus,
        stderr: String,
    },
    CompilerTerminated {
        phase: NativePhase,
        input_path: PathBuf,
        output_path: PathBuf,
        stderr: String,
    },
    CurrentDirectory {
        source: io::Error,
    },
    ExecutableSpawn {
        path: PathBuf,
        source: io::Error,
    },
    ExecutableStatusOutOfRange {
        path: PathBuf,
        status: i32,
    },
    ExecutableTerminated {
        path: PathBuf,
    },
}

impl fmt::Display for NativeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CompilerSpawn { phase, input_path, output_path, source } => {
                write_compiler_context(
                    formatter,
                    *phase,
                    input_path,
                    output_path,
                    "failed to spawn",
                )?;
                write!(formatter, ": {source}")
            }
            Self::CompilerFailure { phase, input_path, output_path, status, stderr } => {
                write_compiler_context(formatter, *phase, input_path, output_path, "failed")?;
                write!(formatter, " with {status}")?;
                write_stderr(formatter, stderr)
            }
            Self::CompilerTerminated { phase, input_path, output_path, stderr } => {
                write_compiler_context(
                    formatter,
                    *phase,
                    input_path,
                    output_path,
                    "terminated without an exit code",
                )?;
                write_stderr(formatter, stderr)
            }
            Self::CurrentDirectory { source } => {
                write!(formatter, "failed to resolve the current directory before launch: {source}")
            }
            Self::ExecutableSpawn { path, source } => {
                write!(formatter, "failed to launch executable '{}': {source}", path.display())
            }
            Self::ExecutableStatusOutOfRange { path, status } => write!(
                formatter,
                "executable '{}' exited with status {status}, which cannot be propagated",
                path.display()
            ),
            Self::ExecutableTerminated { path } => {
                write!(formatter, "executable '{}' terminated without an exit code", path.display())
            }
        }
    }
}

/// Compiles a C source file into an object file.
pub(super) fn compile_c(
    c_path: &Path,
    object_path: &Path,
    optimization_level: OptimizationLevel,
) -> Result<(), NativeError> {
    let output = Command::new("cc")
        .arg("-c")
        .arg(c_path)
        .arg("-o")
        .arg(object_path)
        .arg(optimization_flag(optimization_level))
        .output()
        .map_err(|source| NativeError::CompilerSpawn {
            phase: NativePhase::Compilation,
            input_path: c_path.to_owned(),
            output_path: object_path.to_owned(),
            source,
        })?;

    check_compiler_status(
        NativePhase::Compilation,
        c_path,
        object_path,
        output.status,
        &output.stderr,
    )
}

/// Links an object file into an executable.
pub(super) fn link_executable(
    object_path: &Path,
    executable_path: &Path,
) -> Result<(), NativeError> {
    let output =
        Command::new("cc").arg(object_path).arg("-o").arg(executable_path).output().map_err(
            |source| NativeError::CompilerSpawn {
                phase: NativePhase::Linking,
                input_path: object_path.to_owned(),
                output_path: executable_path.to_owned(),
                source,
            },
        )?;

    check_compiler_status(
        NativePhase::Linking,
        object_path,
        executable_path,
        output.status,
        &output.stderr,
    )
}

/// Launches an executable directly and propagates a representable exit status.
pub(super) fn launch_executable(path: &Path) -> Result<ExitCode, NativeError> {
    let executable_path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|source| NativeError::CurrentDirectory { source })?
            .join(path)
    };

    let status = Command::new(&executable_path)
        .status()
        .map_err(|source| NativeError::ExecutableSpawn { path: executable_path.clone(), source })?;

    if status.success() {
        return Ok(ExitCode::SUCCESS);
    }

    let Some(status) = status.code() else {
        return Err(NativeError::ExecutableTerminated { path: executable_path });
    };

    let status = u8::try_from(status)
        .map_err(|_| NativeError::ExecutableStatusOutOfRange { path: executable_path, status })?;

    Ok(ExitCode::from(status))
}

fn check_compiler_status(
    phase: NativePhase,
    input_path: &Path,
    output_path: &Path,
    status: ExitStatus,
    stderr: &[u8],
) -> Result<(), NativeError> {
    if status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(stderr).into_owned();
    if status.code().is_some() {
        Err(NativeError::CompilerFailure {
            phase,
            input_path: input_path.to_owned(),
            output_path: output_path.to_owned(),
            status,
            stderr,
        })
    } else {
        Err(NativeError::CompilerTerminated {
            phase,
            input_path: input_path.to_owned(),
            output_path: output_path.to_owned(),
            stderr,
        })
    }
}

const fn optimization_flag(optimization_level: OptimizationLevel) -> &'static str {
    match optimization_level {
        OptimizationLevel::O0 => "-O0",
        OptimizationLevel::O1 => "-O1",
        OptimizationLevel::O2 => "-O2",
        OptimizationLevel::O3 => "-O3",
    }
}

fn write_stderr(formatter: &mut fmt::Formatter<'_>, stderr: &str) -> fmt::Result {
    if stderr.trim().is_empty() {
        Ok(())
    } else {
        write!(formatter, "\ncc stderr:\n{}", stderr.trim_end())
    }
}

fn write_compiler_context(
    formatter: &mut fmt::Formatter<'_>,
    phase: NativePhase,
    input_path: &Path,
    output_path: &Path,
    outcome: &str,
) -> fmt::Result {
    write!(
        formatter,
        "{} with `cc` {outcome} while producing '{}' from '{}'",
        phase.description(),
        output_path.display(),
        input_path.display()
    )
}
