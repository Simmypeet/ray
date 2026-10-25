//! Resolves what an invocation compiles from the command line arguments and,
//! for a project, its manifest.

use std::path::{Path, PathBuf};

use rayc_project::{OUTPUT_DIRECTORY_NAME, Project};
use rayc_target::{Arguments, OptimizationLevel, TargetKind};

/// The artifact an invocation produces after a successful check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Artifact {
    kind: TargetKind,
    output_path: Option<PathBuf>,
    optimization_level: OptimizationLevel,
    run: bool,
}

impl Artifact {
    /// Returns the kind of artifact to produce.
    pub(super) const fn kind(&self) -> TargetKind { self.kind }

    /// Returns where the artifact is written, if the artifact kind has a
    /// default output path or one was requested.
    pub(super) fn output_path(&self) -> Option<&Path> { self.output_path.as_deref() }

    /// Returns the optimization level the artifact is compiled with.
    pub(super) const fn optimization_level(&self) -> OptimizationLevel { self.optimization_level }

    /// Returns whether the produced executable should be launched.
    pub(super) const fn should_run(&self) -> bool { self.run }

    /// Returns whether producing the artifact requires an executable entry
    /// point.
    pub(super) const fn requires_entry_point(&self) -> bool {
        match self.kind {
            TargetKind::Executable => true,
            TargetKind::Library | TargetKind::LLvmIR | TargetKind::Object | TargetKind::C => false,
        }
    }
}

/// The compilation an invocation performs.
///
/// Command line arguments always take precedence over the project manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Invocation {
    root_file: PathBuf,
    target_name: String,
    incremental_path: Option<PathBuf>,
    artifact: Option<Artifact>,
}

impl Invocation {
    /// Resolves the invocation described by `arguments`.
    ///
    /// A path to a source file compiles that file on its own. A directory, or
    /// the current directory when no path is given, compiles the project
    /// containing it.
    pub(super) fn resolve(arguments: &Arguments) -> Result<Self, rayc_project::Error> {
        match arguments.path() {
            Some(path) if !path.is_dir() => Ok(Self::resolve_source_file(arguments, path)),
            path => Self::resolve_project(arguments, path.unwrap_or_else(|| Path::new("."))),
        }
    }

    fn resolve_source_file(arguments: &Arguments, path: &Path) -> Self {
        // the target is named after the file when no name is requested
        let target_name = arguments.requested_target_name().map_or_else(
            || path.file_stem().unwrap_or_default().to_string_lossy().into_owned(),
            ToOwned::to_owned,
        );

        // artifacts are written to the current working directory
        let artifact =
            Artifact::resolve(arguments, &target_name, TargetKind::Executable, Path::new(""));

        Self {
            root_file: path.to_path_buf(),
            target_name,
            incremental_path: arguments.incremental_path().map(Path::to_path_buf),
            artifact,
        }
    }

    fn resolve_project(arguments: &Arguments, start: &Path) -> Result<Self, rayc_project::Error> {
        let project = Project::discover(start)?;

        // generated files, including the incremental compilation data, live in
        // the output directory of the project; it is created when written to
        let output_directory = project.directory().join(OUTPUT_DIRECTORY_NAME);

        let target_name = arguments.requested_target_name().unwrap_or(project.name()).to_owned();
        let artifact = Artifact::resolve(
            arguments,
            &target_name,
            project.kind().target_kind(),
            &output_directory,
        );

        Ok(Self {
            root_file: project.root_file().to_path_buf(),
            target_name,
            incremental_path: Some(
                arguments
                    .incremental_path()
                    .map_or_else(|| project.default_incremental_path(), Path::to_path_buf),
            ),
            artifact,
        })
    }

    /// Returns the root source file of the target.
    pub(super) fn root_file(&self) -> &Path { &self.root_file }

    /// Returns the name of the target.
    pub(super) fn target_name(&self) -> &str { &self.target_name }

    /// Returns where the incremental compilation data is stored, if the
    /// compilation is incremental.
    pub(super) fn incremental_path(&self) -> Option<&Path> { self.incremental_path.as_deref() }

    /// Returns the artifact to produce, or `None` if the invocation only
    /// checks the program.
    pub(super) const fn artifact(&self) -> Option<&Artifact> { self.artifact.as_ref() }
}

impl Artifact {
    /// Resolves the artifact requested by `arguments`, falling back to
    /// `default_kind` and writing to `output_directory` by default.
    fn resolve(
        arguments: &Arguments,
        target_name: &str,
        default_kind: TargetKind,
        output_directory: &Path,
    ) -> Option<Self> {
        let optimization_level = arguments.optimization_level()?;
        let kind = arguments.requested_artifact_kind().unwrap_or(default_kind);

        // an explicitly requested path is preserved even for artifact kinds
        // without a default one
        let output_path = arguments.requested_output_path().map(Path::to_path_buf).or_else(|| {
            kind.default_file_name(target_name).map(|name| output_directory.join(name))
        });

        Some(Self { kind, output_path, optimization_level, run: arguments.should_run() })
    }
}
