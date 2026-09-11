//! Runs the same diagnostic query as `ray check` against editor contents.

use std::{path::PathBuf, sync::Arc};

use qbice::{serialize::Plugin, stable_hash::SeededStableHasherBuilder};
use rayc_qbice::{Engine, InMemoryFactory};
use rayc_source_file::{LocalSourceID, SourceFile};
use rayc_target::{Arguments, Input, TargetID};
use tower_lsp::lsp_types::{Diagnostic, Url};

use crate::diagnostic::convert;

#[derive(Debug)]
pub(crate) struct Compiler {
    engine: Arc<Engine>,
    target_id: TargetID,
    path: PathBuf,
}

impl Compiler {
    pub(crate) async fn new(path: PathBuf) -> Self {
        let mut engine =
            Engine::new_with(Plugin::new(), InMemoryFactory, SeededStableHasherBuilder::new(0))
                .await
                .expect("in-memory engine is infallible");

        // Retain the distributed executor registrations, as the CLI does.
        rayc_source_file_impl::black_box();
        rayc_lexical_impl::black_box();
        rayc_syntax_impl::black_box();
        rayc_semantic_element_impl::black_box();
        rayc_ir_builder::black_box();
        engine.register_program(rayc_qbice::RAY_PROGRAM);

        let arguments = Arguments::new_check(Input::builder().file(path.clone()).build());
        let name = arguments.target_name();
        let target_id = TargetID::from_target_name(&name);
        let engine = Arc::new(engine);
        let mut session = engine.input_session().await;
        session
            .set_input(
                rayc_target::LinkKey { target_id },
                session.intern(std::iter::once(TargetID::CORE).collect()),
            )
            .await;
        session
            .set_input(
                rayc_target::AllTargetIDsKey,
                session.intern([target_id, TargetID::CORE].into_iter().collect()),
            )
            .await;
        session
            .set_input(
                rayc_target::MapKey,
                session.intern(
                    [
                        (session.intern_unsized(name), target_id),
                        (session.intern_unsized("core".to_owned()), TargetID::CORE),
                    ]
                    .into_iter()
                    .collect(),
                ),
            )
            .await;
        session.set_input(rayc_target::Key { target_id }, session.intern(arguments)).await;
        session.set_input(rayc_target::IRVerificationKey { target_id }, false).await;

        // Each document has its own engine and single root source. Assign its
        // identity directly so new, unsaved files need not exist on disk.
        session
            .set_input(
                rayc_source_file::StablePathIDKey {
                    path: session.intern_unsized(path.clone()),
                    target_id,
                },
                Ok(LocalSourceID::new(0, 0)),
            )
            .await;

        session.commit().await;
        Self { engine, target_id, path }
    }

    pub(crate) async fn check(&self, uri: &Url, text: &str) -> Vec<Diagnostic> {
        // Updating the source input invalidates dependent queries while retaining
        // the same engine across edits. Editor contents never touch the disk.
        let mut session = self.engine.input_session().await;
        let target_id = self.target_id;
        let path = session.intern_unsized::<std::path::Path, _>(self.path.clone());
        let source = SourceFile::from_str(text, path.clone());
        session.set_input(rayc_source_file::Key { path, target_id }, Ok(source)).await;
        session.commit().await;

        let engine = self.engine.clone().tracked().await;
        let check = engine.query(&rayc_check::Key { target_id }).await;
        let mut diagnostics = check.all_diagnostics().collect::<Vec<_>>();
        diagnostics.sort();
        diagnostics.into_iter().map(|diagnostic| convert(diagnostic, uri, text)).collect()
    }
}

fn test<'a>(mut a: impl FnMut() -> &'a i32) {
    let mut b = a();
    let mut c = a();
}

fn another() {
    let mut a = 2;
    test(|| &a);
}
