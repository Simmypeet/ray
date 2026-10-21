//! Discovers and loads Ray projects described by a `ray.toml` manifest.
//!
//! A project is a directory containing a manifest. The manifest names the
//! target and its kind, and locates the root source file the module tree stems
//! from. Both the compiler and the language server resolve projects through
//! this crate so that they agree on what a project is.

use std::{
    io,
    path::{Path, PathBuf},
    str::FromStr,
};

use rayc_lexical::kind::{Identifier, Keyword};
use rayc_target::TargetKind;
use serde::Deserialize;

/// The file name of the project manifest.
pub const MANIFEST_FILE_NAME: &str = "ray.toml";

/// The directory, relative to the project directory, holding the files the
/// compiler generates for the project.
pub const OUTPUT_DIRECTORY_NAME: &str = "target";

/// The kind of target a project produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Deserialize)]
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

    /// The target name is not usable as the name of the root module.
    #[error(
        "invalid target name `{name}` in '{}': it must be an identifier that is not a keyword",
        path.display()
    )]
    InvalidName {
        /// The path to the manifest.
        path: PathBuf,

        /// The rejected name.
        name: String,
    },

    /// The target name is reserved for the compiler core library.
    #[error(
        "invalid target name `core` in '{}': it is reserved for the compiler core library",
        path.display()
    )]
    ReservedName {
        /// The path to the manifest.
        path: PathBuf,
    },
}

/// The contents of a `ray.toml` manifest.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    target: TargetSection,
}

/// The `[target]` section of a manifest.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetSection {
    name: String,

    #[serde(default)]
    kind: Kind,

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

    fn load_canonical(directory: PathBuf) -> Result<Self, Error> {
        // read and parse the manifest
        let path = directory.join(MANIFEST_FILE_NAME);
        let content = std::fs::read_to_string(&path)
            .map_err(|source| Error::Io { path: path.clone(), source })?;
        let Manifest { target } = toml::from_str(&content)
            .map_err(|source| Error::Parse { path: path.clone(), source: Box::new(source) })?;

        // the target name becomes the name of the root module, so it must be
        // nameable in a path such as `name::item`
        if target.name == "core" {
            return Err(Error::ReservedName { path });
        }
        if !Identifier::is_identifier_string(&target.name)
            || Keyword::from_str(&target.name).is_ok()
        {
            return Err(Error::InvalidName { path, name: target.name });
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
