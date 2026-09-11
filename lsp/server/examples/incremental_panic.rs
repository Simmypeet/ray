//! Exercises declaration edits that previously panicked with a reused engine.

use std::sync::Arc;

use qbice::{serialize::Plugin, stable_hash::SeededStableHasherBuilder};
use rayc_qbice::{Engine, InMemoryFactory};
use rayc_source_file::{LocalSourceID, SourceFile};
use rayc_target::{Arguments, Input, TargetID};

async fn reproduce() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("main.ray");
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

    let versions = [
        "def missingReturn() -> int32:\n    let value = 42\n",
        "def missingReturn() -> int32:\n    return 42\n",
        "def missingReturn() -> int32:\n    let value = 42\n",
        "def",
    ];
    for (index, text) in versions.into_iter().enumerate() {
        eprintln!("Checking version {}: {text:?}", index + 1);
        let mut session = engine.input_session().await;
        let path = session.intern_unsized::<std::path::Path, _>(path.clone());
        let source = SourceFile::from_str(text, path.clone());
        session.set_input(rayc_source_file::Key { path, target_id }, Ok(source)).await;
        session.commit().await;

        // Release the tracked phase before starting the next input session.
        let tracked = engine.clone().tracked().await;
        let check = tracked.query(&rayc_check::Key { target_id }).await;
        eprintln!(
            "Version {} completed: {} diagnostics",
            index + 1,
            check.all_diagnostics().count()
        );
    }
}

fn main() {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(8 * 1024 * 1024)
        .build()
        .unwrap()
        .block_on(reproduce());
}
