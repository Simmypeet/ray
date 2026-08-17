//! Contains the main `run()` function for the compiler.

use std::{io::Write, process::ExitCode, sync::Arc};

use qbice::{serialize::Plugin, stable_hash::SeededStableHasherBuilder};
use rayc_qbice::{Engine, InMemoryFactory, IncrementalStorageEngine};
use rayc_symbol_impl::source_map::create_source_map;
use rayc_target::{Arguments, TargetID};
use tracing::instrument;

use crate::term::ReportTerm;

pub mod term;

async fn create_engine(argument: &Arguments, report_term: &mut ReportTerm<'_>) -> Option<Engine> {
    if let Some(inc_path) = argument.incremental_path() {
        match Engine::new_with(
            Plugin::new(),
            IncrementalStorageEngine(inc_path),
            SeededStableHasherBuilder::new(0),
        )
        .await
        {
            Ok(engine) => Some(engine),
            Err(err) => {
                report_term.report_simple_error(format!(
                    "failed to create incremental engine at '{}': {err}",
                    inc_path.display()
                ));

                None
            }
        }
    } else {
        Some(
            Engine::new_with(Plugin::new(), InMemoryFactory, SeededStableHasherBuilder::new(0))
                .await
                .expect("in-memory is infailable"),
        )
    }
}

/// Runs the program with the given arguments.
#[must_use]
#[allow(clippy::too_many_lines, clippy::needless_pass_by_value)]
#[instrument(skip(err_writer, _out_writer))]
pub async fn run(
    argument: Arguments,
    err_writer: &mut dyn Write,
    _out_writer: &mut dyn Write,
) -> ExitCode {
    let mut report_term = ReportTerm::new(err_writer, argument.fancy());

    let Some(mut engine) = create_engine(&argument, &mut report_term).await else {
        return ExitCode::FAILURE;
    };

    // Due to how rust compiler work, if a crate is linked without having any
    // symbols in it used, the crate will be completely ignored and the
    // static distributed registration will be optimized out, causing the engine
    // to not have the executors
    rayc_source_file_impl::black_box();
    rayc_lexical_impl::black_box();
    rayc_syntax_impl::black_box();
    rayc_semantic_element_impl::black_box();

    engine.register_program(rayc_qbice::RAY_PROGRAM);

    // set the initial input, the invocation arguments
    let target_name = argument.target_name();
    let local_target_id = TargetID::from_target_name(&target_name);

    let engine = Arc::new(engine);

    {
        let mut input_session = engine.input_session().await;

        // rayc_corelib_impl::initialize_corelib(&mut input_session).await;

        input_session
            .set_input(
                rayc_target::LinkKey { target_id: local_target_id },
                input_session.intern(std::iter::once(TargetID::CORE).collect()),
            )
            .await;

        input_session
            .set_input(
                rayc_target::AllTargetIDsKey,
                input_session.intern([local_target_id, TargetID::CORE].into_iter().collect()),
            )
            .await;

        input_session
            .set_input(
                rayc_target::MapKey,
                input_session.intern(
                    [
                        (input_session.intern_unsized(target_name), local_target_id),
                        (input_session.intern_unsized("core".to_owned()), TargetID::CORE),
                    ]
                    .into_iter()
                    .collect(),
                ),
            )
            .await;

        if let Some(explicit_seed) = argument.target_seed() {
            input_session
                .set_input(rayc_target::SeedKey { target_id: local_target_id }, explicit_seed)
                .await;
        }

        input_session
            .set_input(
                rayc_target::Key { target_id: local_target_id },
                input_session.intern(argument.clone()),
            )
            .await;

        input_session
            .set_input(
                rayc_target::IRVerificationKey { target_id: local_target_id },
                argument.verify_ir(),
            )
            .await;

        rayc_source_file_impl::refresh_source_file_executors(&mut input_session).await;

        input_session.commit().await;
    }

    // now the query can start ...

    let tracked_engine = engine.clone().tracked().await;

    let source_map = tracked_engine.create_source_map(local_target_id).await;

    report_term.set_source_map(&source_map);

    let diagnostic_count = {
        let check = tracked_engine.query(&rayc_check::Key { target_id: local_target_id }).await;

        let mut diagnostics: Vec<_> = check.all_diagnostics().collect();
        diagnostics.sort();

        for diag in &diagnostics {
            report_term.report_rendered(diag);
        }

        diagnostics.len()
    };

    if diagnostic_count != 0 {
        report_term
            .report_simple_error(format!("Compilation aborted due to {diagnostic_count} error(s)"));

        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
