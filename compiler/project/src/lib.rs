//! Discovers and loads Ray projects described by a `ray.toml` manifest.
//!
//! A project is a directory containing a manifest. The manifest names the
//! target and its kind, and locates the root source file the module tree stems
//! from. Both the compiler and the language server resolve projects through
//! this crate so that they agree on what a project is.

use std::{
    ffi::OsStr,
    io,
    path::{Path, PathBuf},
    str::FromStr,
};

use rayc_lexical::kind::{Identifier, Keyword};
use rayc_target::TargetKind;
use serde::{Deserialize, Serialize};

/// The file name of the project manifest.
pub const MANIFEST_FILE_NAME: &str = "ray.toml";

/// The directory, relative to the project directory, holding the files the
/// compiler generates for the project.
pub const OUTPUT_DIRECTORY_NAME: &str = "target";

/// The source file a new binary project starts with.
const BINARY_TEMPLATE: &str = "\
extern def printf(format: cstr, ...) -> c_int

def main() -> int32:
    printf(\"Hello, world!\\n\")
    return 0
";

/// The source file a new library project starts with.
const LIBRARY_TEMPLATE: &str = "\
pub def add(left: int32, right: int32) -> int32:
    return left + right
";

/// The ignore file of a new project, keeping generated files out of version
/// control.
const GITIGNORE_TEMPLATE: &str = "/target\n";

/// The kind of target a project produces.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub enum Kind {
    /// An executable with a `main` entry point.
    #[default]
    #[serde(rename = "bin")]
    Binary,

    /// A library that other targets can link to.
    #[serde(rename = "lib")]
    Library,
}

impl Kind {
    /// Returns the artifact the compiler builds for this kind of project.
    #[must_use]
    pub const fn target_kind(self) -> TargetKind {
        match self {
            Self::Binary => TargetKind::Executable,
            Self::Library => TargetKind::Library,
        }
    }

    /// Returns the root source file used when the manifest does not name one,
    /// relative to the project directory.
    #[must_use]
    pub const fn default_root_file(self) -> &'static str {
        match self {
            Self::Binary => "src/main.ray",
            Self::Library => "src/lib.ray",
        }
    }

    /// Returns a human readable description of this kind of project.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Binary => "binary",
            Self::Library => "library",
        }
    }

    /// Returns the contents of the root source file of a new project.
    const fn template(self) -> &'static str {
        match self {
            Self::Binary => BINARY_TEMPLATE,
            Self::Library => LIBRARY_TEMPLATE,
        }
    }
}

/// The reason a name cannot name a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NameError {
    /// The name is not an identifier, or it is a keyword.
    #[error("it must be an identifier that is not a keyword")]
    NotIdentifier,

    /// The name is reserved for the compiler core library.
    #[error("it is reserved for the compiler core library")]
    Reserved,
}

impl NameError {
    /// Checks that `name` can name a target.
    ///
    /// The target name becomes the name of the root module, so it must be
    /// nameable in a path such as `name::item`.
    fn validate(name: &str) -> Result<(), Self> {
        if name == "core" {
            return Err(Self::Reserved);
        }
        if !Identifier::is_identifier_string(name) || Keyword::from_str(name).is_ok() {
            return Err(Self::NotIdentifier);
        }

        Ok(())
    }
}

/// An error encountered while discovering or loading a project.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No manifest exists in the starting directory or any of its parents.
    #[error(
        "could not find `{MANIFEST_FILE_NAME}` in '{}' or any parent directory",
        start.display()
    )]
    ManifestNotFound {
        /// The path the search started from.
        start: PathBuf,
    },

    /// A path could not be resolved or the manifest could not be read.
    #[error("failed to read '{}': {source}", path.display())]
    Io {
        /// The path that failed.
        path: PathBuf,

        /// The underlying IO error.
        source: io::Error,
    },

    /// The manifest is not a well-formed manifest.
    #[error("failed to parse '{}': {source}", path.display())]
    Parse {
        /// The path to the manifest.
        path: PathBuf,

        /// The underlying TOML error.
        source: Box<toml::de::Error>,
    },

    /// The target name in the manifest cannot name a target.
    #[error("invalid target name `{name}` in '{}': {reason}", path.display())]
    InvalidName {
        /// The path to the manifest.
        path: PathBuf,

        /// The rejected name.
        name: String,

        /// Why the name was rejected.
        reason: NameError,
    },

    /// The name chosen for a new project cannot name a target.
    #[error("invalid project name `{name}`: {reason}")]
    InvalidProjectName {
        /// The rejected name.
        name: String,

        /// Why the name was rejected.
        reason: NameError,
    },

    /// A new project was not named and its directory has no name to use.
    #[error("cannot name the project after '{}'", path.display())]
    UnnamedDirectory {
        /// The directory the project was to be created in.
        path: PathBuf,
    },

    /// The directory of a new project already exists.
    #[error("destination '{}' already exists", path.display())]
    AlreadyExists {
        /// The directory the project was to be created in.
        path: PathBuf,
    },

    /// A file of a new project could not be written.
    #[error("failed to write '{}': {source}", path.display())]
    Write {
        /// The path that failed.
        path: PathBuf,

        /// The underlying IO error.
        source: io::Error,
    },
}

impl Error {
    /// Returns whether the error rejects the name of a new project, which
    /// naming the project explicitly can fix.
    #[must_use]
    pub const fn is_project_name_error(&self) -> bool {
        match self {
            Self::InvalidProjectName { .. } | Self::UnnamedDirectory { .. } => true,
            Self::ManifestNotFound { .. }
            | Self::Io { .. }
            | Self::Parse { .. }
            | Self::InvalidName { .. }
            | Self::AlreadyExists { .. }
            | Self::Write { .. } => false,
        }
    }
}

/// The contents of a `ray.toml` manifest.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    target: TargetSection,
}

/// The `[target]` section of a manifest.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetSection {
    name: String,

    #[serde(default)]
    kind: Kind,

    #[serde(skip_serializing_if = "Option::is_none")]
    root: Option<PathBuf>,
}

/// A project loaded from its manifest, with every path made absolute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    directory: PathBuf,
    name: String,
    kind: Kind,
    root_file: PathBuf,
}

impl Project {
    /// Finds the project containing `start`, which is either a file or a
    /// directory.
    ///
    /// The search starts at `start` and walks up its ancestors, loading the
    /// first manifest it finds.
    pub fn discover(start: &Path) -> Result<Self, Error> {
        // resolve symbolic links and relative components first so that the
        // project directory, and every path derived from it, is canonical
        let start = std::fs::canonicalize(start)
            .map_err(|source| Error::Io { path: start.to_path_buf(), source })?;

        let directory = start
            .ancestors()
            .find(|ancestor| ancestor.join(MANIFEST_FILE_NAME).is_file())
            .ok_or_else(|| Error::ManifestNotFound { start: start.clone() })?;

        Self::load_canonical(directory.to_path_buf())
    }

    /// Loads the project whose manifest is in `directory`.
    pub fn load(directory: &Path) -> Result<Self, Error> {
        let directory = std::fs::canonicalize(directory)
            .map_err(|source| Error::Io { path: directory.to_path_buf(), source })?;

        Self::load_canonical(directory)
    }

    /// Creates a new project of the given `kind` in `directory`, which must
    /// not exist yet.
    ///
    /// The project is named after `directory` unless `name` is given. Its
    /// directory contains the manifest, the root source file and an ignore
    /// file for the generated files.
    pub fn create(directory: &Path, name: Option<&str>, kind: Kind) -> Result<Self, Error> {
        // name the project after its directory unless it is named explicitly
        let name = match name {
            Some(name) => name,
            None => directory
                .file_name()
                .and_then(OsStr::to_str)
                .ok_or_else(|| Error::UnnamedDirectory { path: directory.to_path_buf() })?,
        };
        NameError::validate(name)
            .map_err(|reason| Error::InvalidProjectName { name: name.to_owned(), reason })?;

        // refuse an existing directory instead of filling it; creating it
        // would succeed silently
        if directory.exists() {
            return Err(Error::AlreadyExists { path: directory.to_path_buf() });
        }
        std::fs::create_dir_all(directory)
            .map_err(|source| Error::Write { path: directory.to_path_buf(), source })?;

        // write the manifest, the ignore file and the root source file
        let manifest =
            Manifest { target: TargetSection { name: name.to_owned(), kind, root: None } };
        let manifest = toml::to_string(&manifest).expect("the manifest is serializable");
        let root_file = directory.join(kind.default_root_file());

        for (path, contents) in [
            (directory.join(MANIFEST_FILE_NAME), manifest.as_str()),
            (directory.join(".gitignore"), GITIGNORE_TEMPLATE),
            (root_file, kind.template()),
        ] {
            path.parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::write(&path, contents))
                .map_err(|source| Error::Write { path, source })?;
        }

        Self::load(directory)
    }

    fn load_canonical(directory: PathBuf) -> Result<Self, Error> {
        // read and parse the manifest
        let path = directory.join(MANIFEST_FILE_NAME);
        let content = std::fs::read_to_string(&path)
            .map_err(|source| Error::Io { path: path.clone(), source })?;
        let Manifest { target } = toml::from_str(&content)
            .map_err(|source| Error::Parse { path: path.clone(), source: Box::new(source) })?;

        if let Err(reason) = NameError::validate(&target.name) {
            return Err(Error::InvalidName { path, name: target.name, reason });
        }

        // the root file is relative to the project directory
        let root_file = directory
            .join(target.root.unwrap_or_else(|| PathBuf::from(target.kind.default_root_file())));

        Ok(Self { directory, name: target.name, kind: target.kind, root_file })
    }

    /// Returns the canonical directory containing the manifest.
    #[must_use]
    pub fn directory(&self) -> &Path { &self.directory }

    /// Returns the name of the target, which is also the name of its root
    /// module.
    #[must_use]
    pub fn name(&self) -> &str { &self.name }

    /// Returns the kind of target the project produces.
    #[must_use]
    pub const fn kind(&self) -> Kind { self.kind }

    /// Returns the absolute path to the root source file.
    ///
    /// The file is not required to exist; the compiler reports a missing root
    /// file when it loads the module tree.
    #[must_use]
    pub fn root_file(&self) -> &Path { &self.root_file }

    /// Returns the directory holding the incremental compilation data of the
    /// project.
    #[must_use]
    pub fn default_incremental_path(&self) -> PathBuf {
        self.directory.join(OUTPUT_DIRECTORY_NAME).join("inc")
    }
}
