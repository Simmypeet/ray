//! Checks a project with the same diagnostic query as `rayc check`.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Arc,
    time::Duration,
};

use qbice::{serialize::Plugin, stable_hash::SeededStableHasherBuilder};
use rayc_arena::ID;
use rayc_project::Project;
use rayc_qbice::{Engine, InMemoryFactory, IncrementalStorageEngine, InputSession};
use rayc_source_file::ByteIndex;
use rayc_symbol_impl::source_map::create_source_map;
use rayc_target::TargetID;
use tokio::{sync::Mutex, task::JoinHandle};
use tower_lsp::{
    Client,
    lsp_types::{CompletionItem, Diagnostic, Url},
};

use crate::{
    completion,
    diagnostic::Locator,
    overlay::{Overlay, SourceFileExecutor},
};

/// How long edits must settle before a check starts.
const DEBOUNCE: Duration = Duration::from_millis(100);

/// The diagnostics of one document, from one check.
#[derive(Debug, Default)]
struct Publication {
    /// The version of the open document that was checked, or `None` for a
    /// file read from disk.
    version: Option<i32>,
    diagnostics: Vec<Diagnostic>,
}

/// Identifies a [`Session`] in the language server.
pub(crate) type SessionID = ID<Session>;

/// A project the language server checks, with the engine caching its
/// compilation.
#[derive(Debug)]
pub(crate) struct Session {
    project: Project,
    engine: Arc<Engine>,
    target_id: TargetID,

    /// The scheduled or running check.
    check: Option<JoinHandle<()>>,

    /// The documents holding diagnostics from the latest check, which have to
    /// be cleared when they no longer have any.
    published: Arc<Mutex<HashSet<Url>>>,
}

impl Session {
    /// Opens a session for `project`, whose documents are read through
    /// `overlay`.
    ///
    /// The compilation is cached in the project's output directory. If that
    /// cache cannot be opened, for example because another language server
    /// holds it, the session caches in memory and the returned warning
    /// explains why.
    pub(crate) async fn open(project: Project, overlay: Arc<Overlay>) -> (Self, Option<String>) {
        let (mut engine, warning) = Self::create_engine(&project).await;

        // Retain the distributed executor registrations, as the CLI does.
        rayc_source_file_impl::black_box();
        rayc_lexical_impl::black_box();
        rayc_syntax_impl::black_box();
        rayc_semantic_element_impl::black_box();
        rayc_ir_builder::black_box();
        engine.register_program(rayc_qbice::RAY_PROGRAM);

        // source files are read from the open documents first
        engine.register_executor(Arc::new(SourceFileExecutor::new(overlay)));

        let engine = Arc::new(engine);
        let mut input_session = engine.input_session().await;
        let target_id = Self::set_project_inputs(&mut input_session, &project).await;
        rayc_corelib::initialize(&mut input_session).await;
        input_session.commit().await;

        let session = Self {
            project,
            engine,
            target_id,
            check: None,
            published: Arc::new(Mutex::new(HashSet::new())),
        };

        (session, warning)
    }

    async fn create_engine(project: &Project) -> (Engine, Option<String>) {
        let path = project.language_server_incremental_path();

        // the database creates only its own directory, not its parents
        let persistent = match std::fs::create_dir_all(&path) {
            Ok(()) => Engine::new_with(
                Plugin::new(),
                IncrementalStorageEngine(&path),
                SeededStableHasherBuilder::new(0),
            )
            .await
            .map_err(|error| error.to_string()),
            Err(error) => Err(error.to_string()),
        };

        match persistent {
            Ok(engine) => (engine, None),
            Err(error) => {
                let engine = Engine::new_with(
                    Plugin::new(),
                    InMemoryFactory,
                    SeededStableHasherBuilder::new(0),
                )
                .await
                .expect("in-memory engine is infallible");
                let warning = format!(
                    "failed to open the incremental cache at '{}': {error}; the project `{}` will \
                     be checked from scratch after every restart",
                    path.display(),
                    project.name()
                );

                (engine, Some(warning))
            }
        }
    }

    /// Sets the inputs describing the target of `project`, returning its ID.
    async fn set_project_inputs(input_session: &mut InputSession, project: &Project) -> TargetID {
        let target_id = TargetID::from_target_name(project.name());

        // the target links only to the core library
        input_session
            .set_input(
                rayc_target::LinkKey { target_id },
                input_session.intern(std::iter::once(TargetID::CORE).collect()),
            )
            .await;
        input_session
            .set_input(
                rayc_target::AllTargetIDsKey,
                input_session.intern([target_id, TargetID::CORE].into_iter().collect()),
            )
            .await;
        input_session
            .set_input(
                rayc_target::MapKey,
                input_session.intern(
                    [
                        (input_session.intern_unsized(project.name().to_owned()), target_id),
                        (input_session.intern_unsized("core".to_owned()), TargetID::CORE),
                    ]
                    .into_iter()
                    .collect(),
                ),
            )
            .await;

        // the target itself, as described by the manifest
        input_session
            .set_input(
                rayc_target::RootFilePathKey { target_id },
                input_session.intern_unsized(project.root_file().to_path_buf()),
            )
            .await;
        input_session
            .set_input(
                rayc_target::NameKey { target_id },
                input_session.intern_unsized(project.name().to_owned()),
            )
            .await;
        input_session.set_input(rayc_target::IRVerificationKey { target_id }, false).await;

        target_id
    }

    /// Returns the canonical directory of the project this session checks.
    pub(crate) fn directory(&self) -> &Path { self.project.directory() }

    /// Switches the session to `project`, the manifest of its project as now
    /// loaded, if the manifest changed.
    ///
    /// The engine and its cache are kept; a renamed target is checked as a new
    /// target.
    pub(crate) async fn reload_if_changed(&mut self, project: Project) {
        if self.project == project {
            return;
        }

        self.cancel_check().await;

        let mut input_session = self.engine.input_session().await;
        self.target_id = Self::set_project_inputs(&mut input_session, &project).await;
        input_session.commit().await;

        self.project = project;
    }

    /// Schedules a check of the project, replacing the one scheduled or
    /// running.
    ///
    /// The check starts once edits settle, re-reads the source files that
    /// changed in the editor or on disk, and publishes the diagnostics of
    /// every file of the project.
    pub(crate) fn schedule_check(&mut self, client: &Client, overlay: &Arc<Overlay>) {
        let previous = self.check.take();
        let engine = self.engine.clone();
        let target_id = self.target_id;
        let root_file = self.project.root_file().to_path_buf();
        let published = self.published.clone();
        let client = client.clone();
        let overlay = overlay.clone();

        self.check = Some(tokio::spawn(async move {
            // a check still running when the inputs change would wait forever,
            // so it is stopped before anything else
            if let Some(previous) = previous {
                previous.abort();
            }

            tokio::time::sleep(DEBOUNCE).await;

            let publications = Self::check(&engine, target_id, &root_file, &overlay).await;
            Self::publish(&client, &published, publications).await;
        }));
    }

    async fn check(
        engine: &Arc<Engine>,
        target_id: TargetID,
        root_file: &Path,
        overlay: &Overlay,
    ) -> HashMap<Url, Publication> {
        // the versions are taken before the documents are read, so that a
        // document edited meanwhile is published as older than it is, which
        // the editor tolerates, rather than newer
        let versions = overlay.versions_snapshot();

        // re-read the source files, from the editor or from disk
        let mut input_session = engine.input_session().await;
        rayc_source_file_impl::refresh_source_file_executors(&mut input_session).await;
        input_session.commit().await;

        // run the same query as `rayc check`
        let tracked_engine = engine.clone().tracked().await;
        let check = tracked_engine.query(&rayc_check::Key { target_id }).await;
        let mut diagnostics = check.all_diagnostics().collect::<Vec<_>>();
        diagnostics.sort();

        // every open document of the target is published, so that the editor
        // learns that its version was checked even without diagnostics
        let sources = tracked_engine.create_source_map(target_id).await;
        let mut publications = sources
            .0
            .iter()
            .filter(|(id, _)| id.target_id == target_id)
            .filter_map(|(_, source)| versions.get(source.path().as_ref()))
            .map(|(uri, version)| {
                (uri.clone(), Publication { version: Some(*version), diagnostics: Vec::new() })
            })
            .collect::<HashMap<_, _>>();

        // group the diagnostics by the document they are shown in
        let locator = Locator::new(&sources, overlay, root_file);
        for diagnostic in diagnostics {
            let (uri, diagnostic) = locator.convert(diagnostic);
            publications.entry(uri).or_default().diagnostics.push(diagnostic);
        }

        publications
    }

    async fn publish(
        client: &Client,
        published: &Mutex<HashSet<Url>>,
        publications: HashMap<Url, Publication>,
    ) {
        let mut published = published.lock().await;

        // record the documents before publishing to them, so that a check
        // cancelled halfway leaves none of them uncleared
        let current = publications.keys().cloned().collect::<HashSet<_>>();
        let fixed = published.difference(&current).cloned().collect::<Vec<_>>();
        published.extend(current.iter().cloned());

        // clear the documents no longer published, such as closed documents
        // whose diagnostics are all fixed
        for uri in fixed {
            client.publish_diagnostics(uri, Vec::new(), None).await;
        }

        for (uri, publication) in publications {
            client.publish_diagnostics(uri, publication.diagnostics, publication.version).await;
        }

        *published = current;
    }

    /// Lists the completions at `offset`, a byte index in the source file at
    /// `path`, from the contents the editor shows now.
    pub(crate) async fn complete(
        &mut self,
        path: &Path,
        offset: ByteIndex,
        client: &Client,
        overlay: &Arc<Overlay>,
    ) -> Vec<CompletionItem> {
        // updating the inputs would leave a running check stuck, so the check
        // is stopped first, and scheduled again if it had not finished
        let check_pending = self.check.as_ref().is_some_and(|check| !check.is_finished());
        self.cancel_check().await;

        // the completion follows edits that no check has read yet
        let mut input_session = self.engine.input_session().await;
        rayc_source_file_impl::refresh_source_file_executors(&mut input_session).await;
        input_session.commit().await;

        let tracked_engine = self.engine.clone().tracked().await;
        let items = completion::complete(&tracked_engine, self.target_id, path, offset).await;

        if check_pending {
            self.schedule_check(client, overlay);
        }

        items
    }

    async fn cancel_check(&mut self) {
        if let Some(check) = self.check.take() {
            check.abort();
            let _ = check.await;
        }
    }

    /// Stops checking the project, releasing its engine and the incremental
    /// cache.
    pub(crate) async fn stop(mut self) { self.cancel_check().await; }

    /// Stops checking the project and clears its diagnostics.
    pub(crate) async fn close(mut self, client: &Client) {
        self.cancel_check().await;

        for uri in self.published.lock().await.drain() {
            client.publish_diagnostics(uri, Vec::new(), None).await;
        }
    }
}
