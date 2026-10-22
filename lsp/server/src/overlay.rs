//! Serves the contents of open documents to the compiler in place of the
//! files saved on disk.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use qbice::{
    ExecutionStyle, Executor,
    stable_hash::{SeededStableHasherBuilder, Sip128Hasher},
    storage::intern::Interner,
};
use rayc_arena::{Arena, ID};
use rayc_hash::FxHashMap;
use rayc_qbice::{Config, TrackedEngine};
use rayc_source_file::{EditorLocation, SourceFile};
use rayc_source_file_impl::LoadSourceFileExecutor;
use tower_lsp::lsp_types::{Position, TextDocumentContentChangeEvent, Url};

use crate::session::SessionID;

/// Identifies an open document in the [`Overlay`].
pub(crate) type BufferID = ID<Buffer>;

/// An open document.
#[derive(Debug, Clone)]
pub(crate) struct Buffer {
    uri: Url,

    /// The canonical path of the document, the form the compiler uses.
    path: PathBuf,

    version: i32,
    source: SourceFile,

    /// The session checking the project the document belongs to, if any.
    session: Option<SessionID>,
}

/// The open documents and their indexes, which change together.
#[derive(Debug, Default)]
struct Buffers {
    arena: Arena<Buffer>,

    /// The documents by the canonical path the compiler reads them by.
    ids_by_path: FxHashMap<PathBuf, BufferID>,

    /// The documents by the URI the editor refers to them by.
    ids_by_uri: FxHashMap<Url, BufferID>,
}

impl Buffers {
    fn by_path(&self, path: &Path) -> Option<&Buffer> {
        self.ids_by_path.get(path).and_then(|buffer_id| self.arena.get(*buffer_id))
    }
}

/// The documents open in the editor.
///
/// The compiler reads source files through [`SourceFileExecutor`], so it sees
/// the unsaved contents of open documents. The buffers are never set as query
/// inputs: an input would be persisted in the incremental database and keep
/// shadowing the file on disk after the document is closed or the server
/// restarts.
#[derive(Debug)]
pub(crate) struct Overlay {
    /// Interns the paths of the buffered source files.
    ///
    /// The overlay serves the engines of every project, so it cannot use the
    /// interner of one of them; interned values compare by value, so the
    /// engines accept these paths.
    interner: Interner,

    /// The documents, read by the checks while the editor changes them; the
    /// lock is never held across an `await`.
    buffers: RwLock<Buffers>,
}

impl Default for Overlay {
    fn default() -> Self {
        Self {
            // it holds only the paths of open documents; the interner needs a
            // power of two greater than one shards
            interner: Interner::new(2, SeededStableHasherBuilder::<Sip128Hasher>::new(0)),
            buffers: RwLock::default(),
        }
    }
}

impl Overlay {
    /// Returns the canonical form of `path`, the form the compiler uses.
    ///
    /// A document that is not saved yet has no file to resolve, so only its
    /// directory is.
    pub(crate) fn canonical_path(path: &Path) -> PathBuf {
        if let Ok(path) = std::fs::canonicalize(path) {
            return path;
        }

        path.parent()
            .zip(path.file_name())
            .and_then(|(parent, name)| Some(std::fs::canonicalize(parent).ok()?.join(name)))
            .unwrap_or_else(|| path.to_path_buf())
    }

    /// Opens the document at `uri`, whose canonical path is `path`, with the
    /// given contents.
    pub(crate) fn open(
        &self,
        uri: Url,
        path: PathBuf,
        version: i32,
        text: &str,
        session: Option<SessionID>,
    ) -> BufferID {
        let source = SourceFile::from_str(text, self.interner.intern_unsized(path.clone()));

        let mut buffers = self.buffers.write().expect("poisoned");
        let buffer_id = buffers.arena.insert(Buffer {
            uri: uri.clone(),
            path: path.clone(),
            version,
            source,
            session,
        });
        buffers.ids_by_uri.insert(uri, buffer_id);
        buffers.ids_by_path.insert(path, buffer_id);

        buffer_id
    }

    /// Applies the changes of a new `version` of the document the editor
    /// refers to by `uri`, in order.
    ///
    /// A change with a range replaces that range, while a change without one
    /// replaces the whole document. Returns the session of the document, or
    /// `None` if the document is not open, the version is not newer, or the
    /// document is not in a project.
    pub(crate) fn edit_by_uri(
        &self,
        uri: &Url,
        version: i32,
        changes: impl IntoIterator<Item = TextDocumentContentChangeEvent>,
    ) -> Option<SessionID> {
        // the document is found and edited under one lock, so that it cannot
        // be closed in between
        let mut buffers = self.buffers.write().expect("poisoned");
        let buffer_id = *buffers.ids_by_uri.get(uri)?;
        let buffer = buffers.arena.get_mut(buffer_id)?;
        if version <= buffer.version {
            return None;
        }

        // edit the rope in place instead of rebuilding the whole document
        let source = &mut buffer.source;
        for change in changes {
            let range = change.range.map_or(0..source.len_bytes(), |range| {
                Self::byte_index(source, range.start)..Self::byte_index(source, range.end)
            });
            source.replace_range(range, &change.text);
        }

        buffer.version = version;
        buffer.session
    }

    fn byte_index(source: &SourceFile, position: Position) -> usize {
        source.get_byte_index_from_utf16_location(&EditorLocation::new(
            position.line as usize,
            position.character as usize,
        ))
    }

    /// Closes the document the editor refers to by `uri`, so that its file is
    /// read from disk.
    ///
    /// Returns the session the document belonged to, if the document was open
    /// and in a project.
    pub(crate) fn close_by_uri(&self, uri: &Url) -> Option<SessionID> {
        // the document and its indexes are removed under one lock
        let mut buffers = self.buffers.write().expect("poisoned");
        let buffer_id = buffers.ids_by_uri.remove(uri)?;
        let buffer = buffers.arena.remove(buffer_id)?;
        buffers.ids_by_path.remove(&buffer.path);

        buffer.session
    }

    /// Returns the open documents.
    ///
    /// The IDs are collected so that the documents can be updated while going
    /// through them.
    pub(crate) fn buffer_ids(&self) -> Vec<BufferID> {
        self.buffers.read().expect("poisoned").arena.ids().collect()
    }

    /// Returns the canonical path of the document `buffer_id`.
    pub(crate) fn path(&self, buffer_id: BufferID) -> Option<PathBuf> {
        self.buffers
            .read()
            .expect("poisoned")
            .arena
            .get(buffer_id)
            .map(|buffer| buffer.path.clone())
    }

    /// Sets the session checking the project the document `buffer_id` belongs
    /// to.
    pub(crate) fn set_session(&self, buffer_id: BufferID, session_id: Option<SessionID>) {
        if let Some(buffer) = self.buffers.write().expect("poisoned").arena.get_mut(buffer_id) {
            buffer.session = session_id;
        }
    }

    /// Returns the URI the editor uses for the document at `path` and its
    /// version, if the document is open.
    ///
    /// The editor may refer to a file through a symbolic link, so its URI can
    /// differ from the one built from the canonical path.
    pub(crate) fn document(&self, path: &Path) -> Option<(Url, i32)> {
        let buffers = self.buffers.read().expect("poisoned");
        buffers.by_path(path).map(|buffer| (buffer.uri.clone(), buffer.version))
    }

    /// Returns the URI and version of every open document, keyed by path.
    pub(crate) fn versions_snapshot(&self) -> HashMap<PathBuf, (Url, i32)> {
        self.buffers
            .read()
            .expect("poisoned")
            .arena
            .items()
            .map(|buffer| (buffer.path.clone(), (buffer.uri.clone(), buffer.version)))
            .collect()
    }

    /// Returns the contents of the document at `path`, if it is open.
    ///
    /// Cloning a source file shares its rope, so this does not copy the text.
    fn source_file(&self, path: &Path) -> Option<SourceFile> {
        let buffers = self.buffers.read().expect("poisoned");
        buffers.by_path(path).map(|buffer| buffer.source.clone())
    }
}

/// Loads a source file from the open documents, falling back to the file on
/// disk.
///
/// It replaces the executor of the compiler. Like that executor, it is an
/// external input, so refreshing the source files picks up both edits in the
/// editor and changes on disk.
#[derive(Debug)]
pub(crate) struct SourceFileExecutor {
    overlay: Arc<Overlay>,
}

impl SourceFileExecutor {
    /// Creates an executor serving the documents in `overlay`.
    pub(crate) const fn new(overlay: Arc<Overlay>) -> Self { Self { overlay } }
}

impl Executor<rayc_source_file::Key, Config> for SourceFileExecutor {
    async fn execute(
        &self,
        key: &rayc_source_file::Key,
        engine: &TrackedEngine,
    ) -> Result<SourceFile, rayc_source_file::Error> {
        match self.overlay.source_file(&key.path) {
            Some(source) => Ok(source),
            None => LoadSourceFileExecutor.execute(key, engine).await,
        }
    }

    fn execution_style() -> ExecutionStyle { ExecutionStyle::ExternalInput }
}
